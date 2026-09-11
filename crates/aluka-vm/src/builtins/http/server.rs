//! HTTP 服务器与 `ServerResponse`：handler 注册、事件派发与响应写出。
//!
//! 对齐 Node.js 22 LTS 标准（`nodehttp/http.go` 的服务器半边）：
//! - `listen` 成功后激活 `"http"` 事件源，泵内非阻塞 accept + 读 socket；
//! - 完整请求解析后构造 `IncomingMessage`/`ServerResponse` 并派发
//!   `'request'`（构造 handler 亦注册为该监听器）；
//! - 无 handler 时按 Go 行为回 `500 no handler`；
//! - 响应在 `end` 时统一写出（Go 缓冲语义），自动补 `Date`、
//!   `Content-Length` 与嗅探的 `Content-Type`（bodyless 状态码除外）；
//! - 连接复用按 Node `_http_outgoing.js` 判定：请求 `Connection: close`／
//!   HTTP/1.0 无 keep-alive／响应显式设 `close` → 响应写 `Connection: close`
//!   并在落盘后发 FIN；其余写 `Connection: keep-alive`（附
//!   `Keep-Alive: timeout=5`）。HTTP/1.0 客户端不写 `Content-Length`，
//!   响应体以关连接定界（同 Node `useChunkedEncodingByDefault === false`）。

use super::state::{
    self, Conn, ReqDispatch, RespBinding, Server, add_listener, has_listener, next_conn_id,
    read_servers, schedule_task, update_response, with_responses, with_servers,
};
use super::wire;
use crate::builtins::buffer;
use crate::builtins::{BuiltinRegistry, current_receiver, register_handler};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// 创建 Server 实例对象（EventEmitter 表面 + Node 属性），并登记
/// 构造 handler 为 `'request'` 监听器。`http.createServer`/`https.createServer`
/// /`http2.createServer` 共用（Go `newHTTPServerWithTLS`）。
pub(crate) fn create_server_object(vm: &mut Vm, handler: Option<Value>) -> ObjectRef {
    create_server_object_tls(vm, handler, None)
}

/// 创建 Server 实例并携带 TLS 服务端配置（`https.createServer` 真实 TLS 接线）。
pub(crate) fn create_server_object_tls(
    vm: &mut Vm,
    handler: Option<Value>,
    tls: Option<std::sync::Arc<rustls::ServerConfig>>,
) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    with_servers(|servers| {
        servers.push(Server {
            obj: obj.0,
            listener: None,
            tls,
            host: String::new(),
            port: 0,
            listening: false,
            conns: Vec::new(),
        });
    });
    let __s = vm.alloc_string("http:server".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(__s));
    // Node 表面属性（Go 逐项对齐）。
    let _ = vm.set_property(Value::Object(obj), "listening", Value::Boolean(false));
    let _ = vm.set_property(Value::Object(obj), "timeout", Value::Number(0.0));
    let _ = vm.set_property(
        Value::Object(obj),
        "keepAliveTimeout",
        Value::Number(5000.0),
    );
    let _ = vm.set_property(Value::Object(obj), "maxHeadersCount", Value::Null);
    let _ = vm.set_property(Value::Object(obj), "headersTimeout", Value::Number(60000.0));
    let _ = vm.set_property(
        Value::Object(obj),
        "requestTimeout",
        Value::Number(300000.0),
    );
    let _ = vm.set_property(
        Value::Object(obj),
        "maxRequestsPerSocket",
        Value::Number(0.0),
    );
    for method in [
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "listen",
        "close",
        "address",
        "setTimeout",
        "getConnections",
        "closeAllConnections",
        "closeIdleConnections",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http:server.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    if let Some(h) = handler {
        add_listener(obj.0, "request", h, false);
    }
    obj
}

/// 注册服务器命名空间（`http:server`）与响应命名空间（`http:response`）的
/// 全部分派处理器（模块 build 时调用一次）。
pub(crate) fn register_handlers(registry: &mut BuiltinRegistry) {
    for (ns, entries) in [
        (
            "http:server",
            vec![
                ("listen", server_listen as crate::builtins::BuiltinHandler),
                ("close", server_close),
                ("address", server_address),
                ("setTimeout", server_set_timeout),
                ("getConnections", server_get_connections),
                ("closeAllConnections", server_close_all),
                ("closeIdleConnections", server_close_all),
                ("on", instance_on),
                ("addListener", instance_on),
                ("once", instance_once),
                ("off", instance_off),
                ("removeListener", instance_off),
            ],
        ),
        (
            "http:response",
            vec![
                ("writeHead", response_write_head),
                ("write", response_write),
                ("end", response_end),
                ("setHeader", response_set_header),
                ("getHeader", response_get_header),
                ("getHeaders", response_get_headers),
                ("hasHeader", response_has_header),
                ("removeHeader", response_remove_header),
                ("addTrailers", response_add_trailers),
                ("flushHeaders", response_flush_headers),
                ("writeContinue", response_write_continue),
                ("cork", response_noop_self),
                ("uncork", response_noop_self),
                ("setTimeout", response_noop_self),
                ("on", instance_on),
                ("addListener", instance_on),
                ("once", instance_once),
                ("off", instance_off),
                ("removeListener", instance_off),
            ],
        ),
    ] {
        for (method, handler) in entries {
            register_handler(registry, ns, method, handler);
        }
    }
}

// --- 实例事件 ------------------------------------------------------------

/// 实例 `on(event, listener)`（Server/IncomingMessage/ServerResponse 共用）。
fn instance_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            add_listener(r.0, &name, *cb, false);
        }
    }
    Ok(receiver)
}

/// 实例 `once(event, listener)`。
fn instance_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            add_listener(r.0, &name, *cb, true);
        }
    }
    Ok(receiver)
}

/// 实例 `off(event, listener)` / `removeListener`：按回调引用移除首个匹配。
fn instance_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
        let name = vm.format_value(*event);
        state::remove_listener(r.0, &name, *cb);
    }
    Ok(receiver)
}

// --- 服务器方法 ----------------------------------------------------------

/// `server.listen(port[, hostname][, callback])`：绑定非阻塞监听器，
/// 成功后激活 `"http"` 事件源，并经宏任务异步触发 callback 与 `'listening'`。
fn server_listen(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let port_in = super::int_arg(args, 0, 0);
    let mut host = String::new();
    let mut callback: Option<Value> = None;
    if let Some(second) = args.get(1) {
        if super::is_function(vm, *second) {
            callback = Some(*second);
        } else {
            host = vm.format_value(*second);
            if let Some(third) = args.get(2) {
                if super::is_function(vm, *third) {
                    callback = Some(*third);
                }
            }
        }
    }
    let port = if port_in > 0 { port_in as u16 } else { 0 };
    let bind_str = if host.is_empty() {
        format!("0.0.0.0:{port}")
    } else {
        format!("{host}:{port}")
    };
    let (bind_host, bind_port) = match bind_str.rsplit_once(':') {
        Some((h, p)) => (
            h.trim_start_matches('[').trim_end_matches(']').to_owned(),
            p.parse::<u16>().unwrap_or(0),
        ),
        None => (bind_str.clone(), 0),
    };
    match crate::builtins::net::bind_shared_listener(&bind_host, bind_port) {
        Ok(ln) => {
            let _ = ln.set_nonblocking(true);
            let (actual_host, actual_port) = match ln.local_addr() {
                Ok(a) => (a.ip().to_string(), a.port()),
                Err(_) => (host.clone(), port),
            };
            with_servers(|servers| {
                if let Some(s) = servers.iter_mut().find(|s| s.obj == r.0) {
                    s.listener = Some(ln);
                    s.host = actual_host;
                    s.port = actual_port;
                    s.listening = true;
                }
            });
            let _ = vm.set_property(receiver, "listening", Value::Boolean(true));
            vm.activate_event_source("http", super::pump);
            if let Some(cb) = callback {
                schedule_task(vm, cb, 0);
            }
            // Go：`listening` 事件经 PostTask 在 callback 之后发射；
            // 此处入待发射队列，由泵在宏任务排空后发出（顺序一致）。
            state::push_pending_event(receiver, "listening");
            Ok(receiver)
        }
        Err(e) => {
            // Node 22：`listen` 失败**异步**派发 `'error'`，载荷为真 `Error`
            // 实例（`code`/`errno`/`syscall`/`address`/`port`）；无监听器时
            // 由 `state::emit` 按 EventEmitter 语义上抛未捕获异常。
            // `listening` 保持 false。
            let err_obj = crate::builtins::net::alloc_listen_error(vm, &e, &bind_host, bind_port);
            state::push_pending_event_with(receiver, "error", vec![Value::Object(err_obj)]);
            vm.activate_event_source("http", super::pump);
            Ok(receiver)
        }
    }
}

/// `server.close([callback])`：立即置 `listening=false` 并注销监听器；
/// 曾监听过则异步触发 callback，否则同步。事件源随之按需停用。
fn server_close(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let callback = args.first().copied().filter(|v| super::is_function(vm, *v));
    let was_listening = with_servers(|servers| {
        let mut listened = false;
        if let Some(s) = servers.iter_mut().find(|s| s.obj == r.0) {
            listened = s.listening || s.listener.is_some();
            s.listening = false;
            s.listener = None;
            s.conns.clear();
        }
        listened
    });
    let _ = vm.set_property(receiver, "listening", Value::Boolean(false));
    super::sync_event_source(vm);
    match (callback, was_listening) {
        (Some(cb), true) => schedule_task(vm, cb, 0),
        (Some(cb), false) => {
            vm.invoke_callable(cb, Value::Undefined, &[])?;
        }
        (None, _) => {}
    }
    Ok(receiver)
}

/// `server.address()`：未监听返回 `null`，否则 `{address, family, port}`。
fn server_address(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Null);
    };
    let addr = read_servers(|servers| {
        servers
            .iter()
            .find(|s| s.obj == r.0)
            .and_then(|s| s.listening.then(|| (s.host.clone(), s.port)))
    });
    let Some((host, port)) = addr else {
        return Ok(Value::Null);
    };
    let obj = vm.alloc_ordinary();
    let host_str = vm.alloc_string(host);
    let _ = vm.set_property(Value::Object(obj), "address", Value::Object(host_str));
    let family = vm.alloc_string("IPv4".to_owned());
    let _ = vm.set_property(Value::Object(obj), "family", Value::Object(family));
    let _ = vm.set_property(Value::Object(obj), "port", Value::Number(port as f64));
    Ok(Value::Object(obj))
}

/// `server.setTimeout([msecs][, callback])`：更新 `timeout` 属性并同步调用
/// callback（Go 同步语义），返回服务器自身。
fn server_set_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(Value::Number(n)) = args.first() {
        let _ = vm.set_property(receiver, "timeout", Value::Number(*n));
    }
    if let Some(cb) = args.get(1) {
        if super::is_function(vm, *cb) {
            vm.invoke_callable(*cb, Value::Undefined, &[])?;
        }
    }
    Ok(receiver)
}

/// `server.getConnections(cb)`：同步回调 `cb(null, 0)`（Go 语义）。
fn server_get_connections(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(cb) = args.first() {
        if super::is_function(vm, *cb) {
            vm.invoke_callable(*cb, Value::Undefined, &[Value::Null, Value::Number(0.0)])?;
        }
    }
    let receiver = current_receiver();
    Ok(receiver)
}

/// `server.closeAllConnections()` / `closeIdleConnections()`：no-op 返回自身。
fn server_close_all(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

// --- ServerResponse 方法 -------------------------------------------------

/// 头对象 → `(小写名, 值列表)` 列表（数组值展开，跳过 undefined/null）。
fn header_entries(vm: &mut Vm, obj: Value) -> Vec<(String, Vec<String>)> {
    let Value::Object(r) = obj else {
        return Vec::new();
    };
    let props: Vec<(String, Value)> = match vm.heap.get(r.0 as usize) {
        Some(HeapObject::Ordinary { .. }) => vm.own_entries(r.0 as usize),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (k, v) in props {
        let vals = super::header_values(vm, v);
        out.push((k.to_ascii_lowercase(), vals));
    }
    out
}

/// `res.writeHead(statusCode[, statusMessage][, headers])`：设置状态码并
/// 冻结当前头集合为线上头（Go `flushHeadersOnce` 一次性转移语义）。
fn response_write_head(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let status = super::int_arg(args, 0, 200) as u16;
    // (statusCode, headers) / (statusCode, statusMessage, headers)：取首个对象参数。
    let mut frozen: Option<Vec<(String, Vec<String>)>> = None;
    update_response(r.0, |b| {
        b.status = status;
        for a in args.iter().skip(1) {
            if super::is_plain_object(vm, *a) {
                for (k, vals) in header_entries(vm, *a) {
                    b.live.retain(|(n, _)| *n != k);
                    b.live.push((k, vals));
                }
                break;
            }
        }
        b.wire = Some(b.live.clone());
        frozen = b.wire.clone();
    });
    let _ = frozen;
    let _ = vm.set_property(receiver, "statusCode", Value::Number(status as f64));
    Ok(receiver)
}

/// `res.write(chunk[, encoding][, callback])`：缓冲响应体，返回 true。
fn response_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Boolean(false));
    };
    if let Some(chunk) = args.first() {
        let bytes = super::chunk_bytes(vm, *chunk);
        update_response(r.0, |b| {
            if !b.finished {
                b.body.extend_from_slice(&bytes);
            }
        });
    }
    Ok(Value::Boolean(true))
}

/// `res.end([chunk])`：追加尾块后统一写出响应（状态行 + 冻结/活动头 +
/// `Date` + `Content-Length` + 嗅探 `Content-Type`），置 `writableEnded`，
/// 并把 `finish`/`close` 事件排入待发射队列。
fn response_end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let Some(chunk) = args.first() {
        if !matches!(chunk, Value::Undefined | Value::Null) && !super::is_function(vm, *chunk) {
            let bytes = super::chunk_bytes(vm, *chunk);
            update_response(r.0, |b| {
                if !b.finished {
                    b.body.extend_from_slice(&bytes);
                }
            });
        }
    }
    // Go 的 res.statusCode 是 accessor：属性写入直接生效，这里读取属性为准。
    let prop_status = vm
        .get_property(receiver, "statusCode")
        .ok()
        .and_then(|v| match v {
            Value::Number(n) => Some(n as u16),
            _ => None,
        });
    finalize_response(vm, r.0, prop_status)?;
    let _ = vm.set_property(receiver, "writableEnded", Value::Boolean(true));
    state::push_pending_event(receiver, "finish");
    state::push_pending_event(receiver, "close");
    Ok(receiver)
}

/// 汇总并写出响应字节（`end` 路径；连接已消失则静默丢弃）。
///
/// 线上头顺序对齐 Node `_storeHeader`：用户头 → `Date` → `Connection`
/// （必要时附 `Keep-Alive`）→ `Content-Length` → 嗅探的 `Content-Type`。
/// `Connection` 语义同 Node `_http_outgoing.js:500-576`：
/// - 用户已在响应头中显式设置 `Connection` → 原样保留，仅按 `close` token
///   决定是否关闭连接（`matchHeader` 的 `RE_CONN_CLOSE` 判定）；
/// - 否则按 `shouldSendKeepAlive = shouldKeepAlive && (有 CL 定界 ||
///   chunked-by-default)` 决定写 `keep-alive`（附 `Keep-Alive: timeout=N`）
///   还是 `close`，后者置本连接为「最后一次」，写出后关闭。
fn finalize_response(vm: &mut Vm, res_id: u32, prop_status: Option<u16>) -> Result<(), VmError> {
    let _ = vm;
    let Some(binding) = state::response_binding(res_id) else {
        return Ok(());
    };
    if binding.finished {
        return Ok(());
    }
    let state::BindingSnapshot {
        server_obj,
        conn_id,
        status: state_status,
        live,
        wire,
        body,
        should_keep_alive,
        use_chunked_by_default,
        ..
    } = binding;
    let status = prop_status.unwrap_or(state_status);
    // 线上头：writeHead 冻结快照优先，否则当前活动头。
    let mut headers: Vec<(String, String)> = match wire {
        Some(frozen) => frozen.into_iter().map(|(k, v)| (k, v.join(", "))).collect(),
        None => live
            .iter()
            .map(|(k, v)| (k.clone(), v.join(", ")))
            .collect(),
    };
    let has_date = headers.iter().any(|(n, _)| n == "date");
    let has_cl = headers.iter().any(|(n, _)| n == "content-length");
    let has_ct = headers.iter().any(|(n, _)| n == "content-type");
    if !has_date {
        headers.push(("date".to_owned(), wire::http_date_now()));
    }
    // `Connection`：用户已设置则保留；否则按 Node 的 keep-alive 判定补写。
    let last = append_conn_headers(&mut headers, should_keep_alive, use_chunked_by_default);
    if !wire::status_is_bodyless(status) {
        // HTTP/1.0 客户端：Node 的 `useChunkedEncodingByDefault === false`，
        // 既不写 CL 也不写 chunked —— 响应体以关连接定界（实测对齐）。
        if !has_cl && use_chunked_by_default {
            headers.push(("content-length".to_owned(), body.len().to_string()));
        }
        if !body.is_empty() && !has_ct {
            headers.push((
                "content-type".to_owned(),
                wire::sniff_content_type(&body).to_owned(),
            ));
        }
    }
    let bytes = wire::serialize_response(status, &headers, &body);
    write_conn_bytes(server_obj, conn_id, &bytes);
    update_response(res_id, |b| {
        b.finished = true;
    });
    mark_conn_idle(server_obj, conn_id);
    if last {
        mark_conn_close_after_write(server_obj, conn_id);
    }
    Ok(())
}

/// 追加 `Connection`（必要时附 `Keep-Alive`）响应头，返回本响应是否为该连接的
/// 最后一次（Node `res._last`）。对齐 `_http_outgoing.js:520-546`：
///
/// - 用户已显式设置 `Connection` → 原样保留，仅按 `close` token
///   （`RE_CONN_CLOSE`）判定 `_last`；
/// - 否则 `shouldSendKeepAlive = shouldKeepAlive && (已设 Content-Length ||
///   useChunkedEncodingByDefault)`：真则写 `keep-alive`（并在用户未自设
///   `Keep-Alive` 时补 `timeout=<server.keepAliveTimeout/1000>`），假则写
///   `close` 并置 `_last`。
fn append_conn_headers(
    headers: &mut Vec<(String, String)>,
    should_keep_alive: bool,
    use_chunked_by_default: bool,
) -> bool {
    match headers
        .iter()
        .find(|(n, _)| n == "connection")
        .map(|(_, v)| v.clone())
    {
        Some(value) => wire::conn_token(&value, "close"),
        None => {
            let has_cl = headers.iter().any(|(n, _)| n == "content-length");
            if should_keep_alive && (has_cl || use_chunked_by_default) {
                headers.push(("connection".to_owned(), "keep-alive".to_owned()));
                if !headers.iter().any(|(n, _)| n == "keep-alive") {
                    // `server.keepAliveTimeout` 默认 5000ms（`_http_server.js:486`）。
                    headers.push(("keep-alive".to_owned(), "timeout=5".to_owned()));
                }
                false
            } else {
                headers.push(("connection".to_owned(), "close".to_owned()));
                true
            }
        }
    }
}

/// 标记连接在本轮响应写完后关闭（Node `res._last` → `socket.destroySoon()`）。
/// 实际 FIN 由泵在 `out` 全部落盘后发出。
fn mark_conn_close_after_write(server_obj: u32, conn_id: u64) {
    with_servers(|servers| {
        if let Some(s) = servers.iter_mut().find(|s| s.obj == server_obj) {
            if let Some(c) = s.conns.iter_mut().find(|c| c.id == conn_id) {
                c.close_after_write = true;
            }
        }
    });
}

/// 把响应字节写入连接（`WouldBlock` 残留进 `out`，泵轮补写）。
///
/// TLS 连接：明文经会话 writer 编码后 `write_tls` 直发（残留密文由
/// rustls 内部保留，io_round 的 flush 段每轮续发）。
fn write_conn_bytes(server_obj: u32, conn_id: u64, bytes: &[u8]) {
    use std::io::Write;
    with_servers(|servers| {
        let Some(s) = servers.iter_mut().find(|s| s.obj == server_obj) else {
            return;
        };
        let Some(conn) = s.conns.iter_mut().find(|c| c.id == conn_id) else {
            return;
        };
        if let Some(tls) = conn.tls.as_mut() {
            if tls.writer().write_all(bytes).is_ok() {
                let _ = tls.writer().flush();
                // 尽力直发（非阻塞；失败/部分由泵 flush 段续写）
                let _ = tls.write_tls(&mut conn.stream);
            }
            return;
        }
        let mut payload: Vec<u8> = Vec::with_capacity(conn.out.len() + bytes.len());
        payload.extend_from_slice(&conn.out);
        payload.extend_from_slice(bytes);
        conn.out.clear();
        match conn.stream.write(&payload) {
            Ok(n) if n < payload.len() => conn.out.extend_from_slice(&payload[n..]),
            Ok(_) => {}
            Err(_) => conn.out = payload,
        }
    });
}

/// 请求处理完成后解除连接的占用标记（恢复 keep-alive 解析）。
fn mark_conn_idle(server_obj: u32, conn_id: u64) {
    with_servers(|servers| {
        if let Some(s) = servers.iter_mut().find(|s| s.obj == server_obj) {
            if let Some(c) = s.conns.iter_mut().find(|c| c.id == conn_id) {
                c.res_active = false;
            }
        }
    });
}

/// `res.setHeader(name, value)`（小写键；数组值保留为多值）。
fn response_set_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let name = vm.format_value(args[0]).to_ascii_lowercase();
        let vals = super::header_values(vm, args[1]);
        update_response(r.0, |b| {
            b.live.retain(|(n, _)| *n != name);
            b.live.push((name, vals));
        });
    }
    Ok(receiver)
}

/// `res.getHeader(name)`：单个值（多值 `", "` 连接）；缺失 `undefined`。
fn response_get_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let Some(name) = args.first() else {
        return Ok(Value::Undefined);
    };
    let name = vm.format_value(*name).to_ascii_lowercase();
    let found = update_response(r.0, |b| {
        b.live
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.join(", "))
    })
    .flatten();
    Ok(found
        .map(|s| Value::Object(vm.alloc_string(s)))
        .unwrap_or(Value::Undefined))
}

/// `res.getHeaders()`：`{小写名: 值|数组}` 对象。
fn response_get_headers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Object(vm.alloc_ordinary()));
    };
    let entries = update_response(r.0, |b| b.live.clone()).unwrap_or_default();
    let obj = vm.alloc_ordinary();
    for (name, vals) in entries {
        let value = if vals.len() == 1 {
            Value::Object(vm.alloc_string(vals[0].clone()))
        } else {
            let elems: Vec<Value> = vals
                .iter()
                .map(|v| Value::Object(vm.alloc_string(v.clone())))
                .collect();
            Value::Object(vm.alloc_array(elems))
        };
        let _ = vm.set_property(Value::Object(obj), &name, value);
    }
    Ok(Value::Object(obj))
}

/// `res.hasHeader(name)`。
fn response_has_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Boolean(false));
    };
    let Some(name) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let name = vm.format_value(*name).to_ascii_lowercase();
    let found = update_response(r.0, |b| b.live.iter().any(|(n, _)| *n == name)).unwrap_or(false);
    Ok(Value::Boolean(found))
}

/// `res.removeHeader(name)`（仅活动头；线上头已冻结不受影响——Go 同）。
fn response_remove_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let Some(name) = args.first() {
        let name = vm.format_value(*name).to_ascii_lowercase();
        update_response(r.0, |b| b.live.retain(|(n, _)| *n != name));
    }
    Ok(receiver)
}

/// `res.addTrailers(headers)`：记录头（aluka 小响应不 chunked，trailer 不上线，
/// 与 Go 缓冲响应行为一致）。
fn response_add_trailers(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(obj) = args.first() {
        let _ = header_entries(vm, *obj);
    }
    Ok(receiver)
}

/// `res.flushHeaders()`：冻结当前头集合（语义等价 `writeHead(status)` 的快照）。
fn response_flush_headers(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    update_response(r.0, |b| {
        if b.wire.is_none() {
            b.wire = Some(b.live.clone());
        }
    });
    Ok(receiver)
}

/// `res.writeContinue()`：no-op 返回自身（Go 简化语义）。
fn response_write_continue(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `res.cork/uncork/setTimeout`：no-op 返回自身。
fn response_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

// --- 泵（服务器半边） ------------------------------------------------------

/// 服务器事件泵：发射待发事件 → flush 待写字节 → accept 新连接 →
/// 读 socket 解析请求并派发。返回本轮是否有进展。
pub(crate) fn pump_servers(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    // 1. 已排队事件（finish/close/listening/error 等，对齐 Go PostTask 时序）
    for (target, event, args) in state::drain_pending_events() {
        state::emit(vm, target, event, &args)?;
        progressed = true;
    }
    // 2. I/O：flush / accept / read / parse（锁内，不触碰 vm）
    let (conn_events, dispatches) = io_round();
    for server_val in conn_events {
        state::emit(vm, server_val, "connection", &[])?;
        progressed = true;
    }
    // 3. 锁外派发请求（构造对象 + handler + 体事件）
    for d in dispatches {
        dispatch_request(vm, d)?;
        progressed = true;
        // handler 期间可能 end()，其 finish/close 事件在此立即发射
        // （Go：PostTask 顺序保证 finish/close 先于 data/end 之后的任务）。
        for (target, event, args) in state::drain_pending_events() {
            state::emit(vm, target, event, &args)?;
        }
    }
    Ok(progressed)
}

/// 一轮 I/O：返回（新连接事件目标，待派发请求）。
fn io_round() -> (Vec<Value>, Vec<ReqDispatch>) {
    use std::io::{Read, Write};
    let mut conn_events: Vec<Value> = Vec::new();
    let mut dispatches: Vec<ReqDispatch> = Vec::new();
    with_servers(|servers| {
        for s in servers.iter_mut() {
            // flush 残留写出（明文路径写 conn.out；TLS 路径把 rustls 内部
            // 待发密文 write_tls 到 socket）
            {
                for conn in s.conns.iter_mut() {
                    let plain_pending = !conn.out.is_empty();
                    if let Some(tls) = conn.tls.as_mut() {
                        if plain_pending {
                            // 明文残留先喂给 TLS writer（write_all 到内部缓冲）
                            let _ = tls.writer().write_all(&conn.out);
                            conn.out.clear();
                        }
                        // 编码明文 + 发送全部待发 TLS 记录（WouldBlock 留内部，下轮续）
                        let _ = tls.writer().flush();
                        loop {
                            match tls.write_tls(&mut conn.stream) {
                                Ok(0) => break,
                                Ok(_) => {}
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => break,
                            }
                        }
                    } else if plain_pending {
                        if let Ok(n) = conn.stream.write(&conn.out) {
                            conn.out.drain(..n);
                        }
                    }
                    // 响应判定为最后一次（`res._last`）：待写字节全部落盘后发 FIN
                    // （Node `socket.destroySoon()`；此处以 shutdown 写方向等价实现）。
                    // TLS 需额外确认 rustls 无待发密文——`out` 清空只代表明文已喂给
                    // 会话 writer，密文仍可能滞留在 rustls 内部缓冲。
                    if conn.close_after_write && !conn.fin_sent && conn.out.is_empty() {
                        match conn.tls.as_mut() {
                            Some(tls) if tls.wants_write() => {}
                            Some(tls) => {
                                tls.send_close_notify();
                                loop {
                                    match tls.write_tls(&mut conn.stream) {
                                        Ok(0) => break,
                                        Ok(_) => {}
                                        Err(ref e)
                                            if e.kind() == std::io::ErrorKind::WouldBlock =>
                                        {
                                            break;
                                        }
                                        Err(_) => break,
                                    }
                                }
                                conn.fin_sent = true;
                            }
                            None => {
                                let _ = conn.stream.shutdown(std::net::Shutdown::Write);
                                conn.fin_sent = true;
                            }
                        }
                    }
                }
            }
            // accept 全部待决连接
            let mut new_conns: usize = 0;
            if let Some(ln) = &s.listener {
                while let Ok((stream, _)) = ln.accept() {
                    let _ = stream.set_nonblocking(true);
                    // TLS 服务器：立即建服务端会话（握手由读段 read_tls 驱动）
                    let tls = match &s.tls {
                        Some(cfg) => rustls::ServerConnection::new(cfg.clone()).ok(),
                        None => None,
                    };
                    s.conns.push(Conn {
                        id: next_conn_id(),
                        stream,
                        tls,
                        buf: Vec::new(),
                        out: Vec::new(),
                        eof: false,
                        res_active: false,
                        close_after_write: false,
                        fin_sent: false,
                    });
                    new_conns += 1;
                }
            }
            if new_conns > 0 {
                conn_events.push(Value::Object(aluka_core::ObjectRef(s.obj)));
            }
            // 读 + 解析
            let mut closed_idx: Vec<usize> = Vec::new();
            for ci in 0..s.conns.len() {
                if !s.conns[ci].res_active {
                    if s.conns[ci].tls.is_some() {
                        // TLS：read_tls → process_new_packets（握手/记录解密）→
                        // 明文读入 buf（握手未完成不解析）
                        let mut tls_done = false;
                        {
                            let conn = &mut s.conns[ci];
                            let tls = conn.tls.as_mut().expect("tls.is_some 已判定");
                            loop {
                                match tls.read_tls(&mut conn.stream) {
                                    Ok(0) => {
                                        conn.eof = true;
                                        break;
                                    }
                                    Ok(_) => {
                                        if tls.process_new_packets().is_err() {
                                            conn.eof = true;
                                            break;
                                        }
                                    }
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => {
                                        conn.eof = true;
                                        break;
                                    }
                                }
                            }
                            if !tls.is_handshaking() {
                                tls_done = true;
                            }
                        }
                        if tls_done && !s.conns[ci].eof {
                            loop {
                                let mut tmp = [0u8; 8192];
                                let conn = &mut s.conns[ci];
                                let tls = conn.tls.as_mut().expect("tls_done 已判定");
                                match tls.reader().read(&mut tmp) {
                                    Ok(0) => {
                                        conn.eof = true;
                                        break;
                                    }
                                    Ok(n) => conn.buf.extend_from_slice(&tmp[..n]),
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => {
                                        conn.eof = true;
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        loop {
                            let mut tmp = [0u8; 8192];
                            match s.conns[ci].stream.read(&mut tmp) {
                                Ok(0) => {
                                    s.conns[ci].eof = true;
                                    break;
                                }
                                Ok(n) => s.conns[ci].buf.extend_from_slice(&tmp[..n]),
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => {
                                    s.conns[ci].eof = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if s.conns[ci].res_active {
                    continue;
                }
                // 可能在同一轮读到多个请求；逐个解析（body 完整才派发）
                while let Some((head, body)) = wire::try_take_request(&mut s.conns[ci].buf) {
                    s.conns[ci].res_active = true;
                    dispatches.push(build_dispatch(s.obj, s.conns[ci].id, head, body));
                }
                // 对端已关闭、或本端已发 FIN（最后一次性响应），且无待处理请求与
                // 残留待写字节时回收连接（同 Node `socket.destroySoon()`：flush
                // 完成后即销毁；`out` 守卫避免截断 WouldBlock 残留的尾字节）。
                if (s.conns[ci].eof || s.conns[ci].fin_sent)
                    && !s.conns[ci].res_active
                    && s.conns[ci].out.is_empty()
                {
                    closed_idx.push(ci);
                }
            }
            for ci in closed_idx.into_iter().rev() {
                s.conns.remove(ci);
            }
        }
    });
    (conn_events, dispatches)
}

/// 由解析结果构造派发项（JS 可见头：小写、剔除 host/transfer-encoding、
/// content-length>0 时补键——Go `newIncomingMessage` 语义）。
fn build_dispatch(
    server_obj: u32,
    conn_id: u64,
    head: wire::RequestHead,
    body: Vec<u8>,
) -> ReqDispatch {
    let mut headers: Vec<(String, Vec<String>)> = Vec::new();
    for (name, vals) in &head.headers {
        if name == "host" || name == "transfer-encoding" {
            continue;
        }
        headers.push((name.clone(), vals.clone()));
    }
    if let Some(cl) = head.content_length {
        if cl > 0 {
            headers.retain(|(n, _)| n != "content-length");
            headers.push(("content-length".to_owned(), vec![cl.to_string()]));
        }
    }
    let (should_keep_alive, use_chunked_by_default) = request_conn_policy(&head);
    ReqDispatch {
        server_obj,
        conn_id,
        method: head.method,
        target: head.target,
        headers,
        body,
        should_keep_alive,
        use_chunked_by_default,
    }
}

/// 请求侧连接策略（Node `parserOnIncoming` 的两个入参）：
///
/// - `should_keep_alive`：llhttp `shouldKeepAlive` 口径——
///   `(Connection 含 keep-alive || HTTP/1.1) && !Connection 含 close`；
/// - `use_chunked_by_default`：`ServerResponse` 构造口径——HTTP/1.0 请求为
///   `Transfer-Encoding: chunked` 时才置真，否则假（响应改以关连接定界）。
fn request_conn_policy(head: &wire::RequestHead) -> (bool, bool) {
    let conn_value = wire::header_value(&head.headers, "connection");
    let has_close = conn_value
        .as_deref()
        .is_some_and(|v| wire::conn_token(v, "close"));
    let has_keep_alive = conn_value
        .as_deref()
        .is_some_and(|v| wire::conn_token(v, "keep-alive"));
    let is_http11 = head.version >= (1, 1);
    let should_keep_alive = (has_keep_alive || is_http11) && !has_close;
    let use_chunked_by_default = is_http11 || head.chunked;
    (should_keep_alive, use_chunked_by_default)
}

/// 派发一个请求：构造 req/res 对象 → `'request'`（或 Go 的 500 兜底）→
/// 微任务收口 → 体事件 `'data'`/`'end'`。
fn dispatch_request(vm: &mut Vm, d: ReqDispatch) -> Result<(), VmError> {
    let req_val = super::build_message_instance(vm, &d.method, &d.target, &d.headers);
    let res_val = build_response_instance(
        vm,
        d.server_obj,
        d.conn_id,
        d.should_keep_alive,
        d.use_chunked_by_default,
    );
    let server_val = Value::Object(aluka_core::ObjectRef(d.server_obj));
    if has_listener(d.server_obj, "request") {
        state::emit(vm, server_val, "request", &[req_val, res_val])?;
    } else {
        // Go：无 handler 时 `WriteHeader(500)` + `"no handler"`（该路径为 Go
        // 语义补位，Node 无对应行为；连接语义仍按请求侧判定对齐）。
        let body = b"no handler";
        let mut headers = vec![("date".to_owned(), wire::http_date_now())];
        let last = append_conn_headers(&mut headers, d.should_keep_alive, d.use_chunked_by_default);
        headers.push(("content-length".to_owned(), body.len().to_string()));
        headers.push((
            "content-type".to_owned(),
            wire::sniff_content_type(body).to_owned(),
        ));
        let bytes = wire::serialize_response(500, &headers, body);
        write_conn_bytes(d.server_obj, d.conn_id, &bytes);
        mark_conn_idle(d.server_obj, d.conn_id);
        if last {
            mark_conn_close_after_write(d.server_obj, d.conn_id);
        }
    }
    // Go：handler 返回后 FlushMicrotasks，再发射体事件。
    vm.drain_microtasks()?;
    if !d.body.is_empty() {
        let chunk = Value::Object(buffer::create_buffer_instance(vm, d.body.clone()));
        state::emit(vm, req_val, "data", &[chunk])?;
    }
    state::emit(vm, req_val, "end", &[])?;
    Ok(())
}

/// 构造 `ServerResponse` 实例并绑定连接（响应写出通道）。
fn build_response_instance(
    vm: &mut Vm,
    server_obj: u32,
    conn_id: u64,
    should_keep_alive: bool,
    use_chunked_by_default: bool,
) -> Value {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("http:response".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    let _ = vm.set_property(Value::Object(obj), "statusCode", Value::Number(200.0));
    let _ = vm.set_property(Value::Object(obj), "writableEnded", Value::Boolean(false));
    for method in [
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "writeHead",
        "write",
        "end",
        "setHeader",
        "getHeader",
        "getHeaders",
        "hasHeader",
        "removeHeader",
        "addTrailers",
        "flushHeaders",
        "writeContinue",
        "cork",
        "uncork",
        "setTimeout",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http:response.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    with_responses(|map| {
        map.insert(
            obj.0,
            RespBinding {
                server_obj,
                conn_id,
                status: 200,
                live: Vec::new(),
                wire: None,
                body: Vec::new(),
                finished: false,
                should_keep_alive,
                use_chunked_by_default,
            },
        );
    });
    Value::Object(obj)
}
