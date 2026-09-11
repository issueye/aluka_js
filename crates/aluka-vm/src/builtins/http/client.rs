//! HTTP 客户端：`ClientRequest` 实例方法与连接泵。
//!
//! 对齐 Node.js 22 LTS 标准（`nodehttp/http.go` 的客户端半边 + `http_agent.go`）：
//! - options 解析逐字复刻（含对象 options 路径被重复拼接的既有行为）；
//! - 请求线上格式对齐 Go `net/http`：`Host`/`User-Agent: Go-http-client/1.1`/
//!   `Accept-Encoding: gzip`，有 body 时 `Transfer-Encoding: chunked`；
//! - 响应回调后同步发射 `'data'`（Buffer）/`'end'`；
//! - `abort`/`destroy` 后按 Node 语义派发 `'error'`(ECONNRESET) + `'close'`。

use super::state::{self, ClientReq, Stage, schedule_task, with_clients};
use super::wire;
use crate::builtins::buffer;
use crate::builtins::{BuiltinRegistry, current_receiver, register_handler};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use std::net::TcpStream;

/// 注册 `http:request` 实例命名空间的分派处理器（模块 build 时调用）。
pub(crate) fn register_handlers(registry: &mut BuiltinRegistry) {
    for (method, handler) in [
        ("write", client_write as crate::builtins::BuiltinHandler),
        ("end", client_end),
        ("setHeader", client_set_header),
        ("getHeader", client_get_header),
        ("getHeaders", client_get_headers),
        ("hasHeader", client_has_header),
        ("removeHeader", client_remove_header),
        ("flushHeaders", client_noop_self),
        ("setTimeout", client_set_timeout),
        ("setNoDelay", client_noop_self),
        ("setSocketKeepAlive", client_noop_self),
        ("abort", client_abort),
        ("destroy", client_destroy),
        ("on", client_on),
        ("addListener", client_on),
        ("once", client_once),
        ("off", client_off),
        ("removeListener", client_off),
    ] {
        register_handler(registry, "http:request", method, handler);
    }
    register_handler(registry, "http:internal", "timeoutEmit", timeout_emit);
}

/// 构造 `ClientRequest` 实例对象（Go `newClientRequestProto`）。
/// `proto` 为默认协议前缀（`"http"`/`"https"`），https 模块复用。
pub(crate) fn create_request_object(
    vm: &mut Vm,
    args: &[Value],
    proto: &str,
) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("http:request".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in [
        "write",
        "end",
        "setHeader",
        "getHeader",
        "getHeaders",
        "hasHeader",
        "removeHeader",
        "flushHeaders",
        "setTimeout",
        "setNoDelay",
        "setSocketKeepAlive",
        "abort",
        "destroy",
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http:request.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    // options 解析（Go 逐字对齐，含 path 双重拼接行为）。
    let mut method = "GET".to_owned();
    let mut url = String::new();
    let mut user_headers: Vec<(String, String)> = Vec::new();
    if let Some(opt) = args.first() {
        match opt {
            _ if super::is_plain_object(vm, *opt) => {
                let get = |vm: &mut Vm, key: &str| -> Option<String> {
                    match vm.get_property(*opt, key) {
                        Ok(v) if !matches!(v, Value::Undefined | Value::Null) => {
                            let s = vm.format_value(v);
                            (!s.is_empty()).then_some(s)
                        }
                        _ => None,
                    }
                };
                if let Some(href) = get(vm, "href") {
                    url = href;
                }
                if let Some(m) = get(vm, "method") {
                    method = m;
                }
                if url.is_empty() {
                    // Node 语义：options 未给 host 时默认 localhost
                    // （Go oracle 对齐；真实包 http.request({port,path}) 形态）
                    let h = get(vm, "host").unwrap_or_else(|| "localhost".to_owned());
                    url = format!("{proto}://{h}");
                    if let Some(p) = get(vm, "port") {
                        url = format!("{}:{p}", url.trim_end_matches('/'));
                    }
                    if let Some(pa) = get(vm, "path") {
                        url = format!("{}{pa}", url.trim_end_matches('/'));
                    }
                }
                // path 已在 url 构造中拼入（if url.is_empty() 分支内），
                // 此处不再重复拼接——此前重复拼接导致 `req.url` 路径翻倍
                // （`/echo/aluka42` → `/echo/aluka42/echo/aluka42`）
                if let Ok(hobj_v) = vm.get_property(*opt, "headers") {
                    if super::is_plain_object(vm, hobj_v) {
                        let hobj = match hobj_v.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_plain_object 已判定为 Ordinary"),
                        };
                        let props: Vec<(String, Value)> = match vm.heap.get(hobj.0 as usize) {
                            Some(HeapObject::Ordinary { .. }) => vm.own_entries(hobj.0 as usize),
                            _ => Vec::new(),
                        };
                        for (k, v) in props {
                            user_headers.retain(|(n, _)| n != &k);
                            user_headers.push((k, vm.format_value(v)));
                        }
                    }
                }
            }
            other => url = vm.format_value(*other),
        }
    }
    // Node 语义：callback 取最后一个函数参数（Go 从 args[1..] 反向找）。
    let callback = args[1.min(args.len())..]
        .iter()
        .rev()
        .find(|v| super::is_function(vm, **v))
        .copied();
    // URL → 目标四元组；解析失败走 Go 的 `NewRequest` 错误路径。
    let (host, port, path, default_port) = match split_url(&url, proto) {
        Some(t) => t,
        None => {
            let msg = format!("http: invalid request URL \"{url}\"");
            if let Some(cb) = callback {
                let err = vm.alloc_string(msg);
                vm.invoke_callable(
                    cb,
                    Value::Undefined,
                    &[Value::Undefined, Value::Object(err)],
                )?;
            }
            return Ok(Value::Object(obj));
        }
    };
    let host_header = if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    with_clients(|clients| {
        clients.push(ClientReq {
            obj: obj.0,
            method,
            host,
            port,
            host_header,
            path,
            headers: user_headers,
            body: Vec::new(),
            callback,
            ended: false,
            aborted: false,
            error_dispatched: false,
            stage: Stage::Connecting,
            tls: proto == "https",
            tls_conn: None,
            stream: None,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            eof: false,
        });
    });
    Ok(Value::Object(obj))
}

/// 拆分 `http(s)://host[:port][/path]`；无协议前缀返回 None（Go 解析错误）。
/// 返回 (主机, 端口, 路径, 协议默认端口)。
fn split_url(url: &str, proto: &str) -> Option<(String, u16, String, u16)> {
    let (rest, default_port) = if let Some(r) = url.strip_prefix("https://") {
        (r, 443u16)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, 80u16)
    } else {
        // Go `NewRequest` 需要绝对 URL；无 scheme 视作解析失败。
        return None;
    };
    let _ = proto;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (authority.to_string(), default_port),
    };
    Some((host, port, path, default_port))
}

// --- ClientRequest 实例方法 ----------------------------------------------

/// `req.write(chunk)`：累积请求体。
fn client_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Boolean(false));
    };
    if let Some(chunk) = args.first() {
        let bytes = super::chunk_bytes(vm, *chunk);
        with_clients(|clients| {
            if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
                c.body.extend_from_slice(&bytes);
            }
        });
    }
    Ok(Value::Boolean(true))
}

/// `req.end([chunk])`：追加尾块并把请求转入泵（发送）。
fn client_end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    if let Some(chunk) = args.first() {
        // Go：仅排除 undefined 与函数（null 会按 "null" 追加，逐字对齐）。
        if !chunk.is_undefined() && !super::is_function(vm, *chunk) {
            let bytes = super::chunk_bytes(vm, *chunk);
            with_clients(|clients| {
                if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
                    c.body.extend_from_slice(&bytes);
                }
            });
        }
    }
    with_clients(|clients| {
        if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
            if !c.ended {
                c.ended = true;
            }
        }
    });
    // 客户端请求已 end：确保事件泵活跃（独立 https.request 无本进程
    // server 时也须推进拨号/握手/响应——M3.2 接线）
    super::sync_event_source(vm);
    Ok(receiver)
}

/// `req.setHeader(name, value)`。
fn client_set_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let name = vm.format_value(args[0]);
        let value = vm.format_value(args[1]);
        with_clients(|clients| {
            if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
                c.headers.retain(|(n, _)| n != &name);
                c.headers.push((name, value));
            }
        });
    }
    Ok(receiver)
}

/// `req.getHeader(name)`（精确键匹配，Go map 语义）。
fn client_get_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Undefined);
    };
    let Some(name) = args.first() else {
        return Ok(Value::Undefined);
    };
    let name = vm.format_value(*name);
    let found = with_clients(|clients| {
        clients
            .iter()
            .find(|c| c.obj == r.0)
            .and_then(|c| c.headers.iter().find(|(n, _)| *n == name))
            .map(|(_, v)| v.clone())
    });
    Ok(found
        .map(|s| Value::Object(vm.alloc_string(s)))
        .unwrap_or(Value::Undefined))
}

/// `req.getHeaders()`：当前请求头对象（精确键）。
fn client_get_headers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Object(vm.alloc_ordinary()));
    };
    let headers = with_clients(|clients| {
        clients
            .iter()
            .find(|c| c.obj == r.0)
            .map(|c| c.headers.clone())
            .unwrap_or_default()
    });
    let obj = vm.alloc_ordinary();
    for (k, v) in headers {
        let val = vm.alloc_string(v);
        let _ = vm.set_property(Value::Object(obj), &k, Value::Object(val));
    }
    Ok(Value::Object(obj))
}

/// `req.hasHeader(name)`。
fn client_has_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Boolean(false));
    };
    let Some(name) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let name = vm.format_value(*name);
    let found = with_clients(|clients| {
        clients
            .iter()
            .find(|c| c.obj == r.0)
            .is_some_and(|c| c.headers.iter().any(|(n, _)| *n == name))
    });
    Ok(Value::Boolean(found))
}

/// `req.removeHeader(name)`。
fn client_remove_header(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    if let Some(name) = args.first() {
        let name = vm.format_value(*name);
        with_clients(|clients| {
            if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
                c.headers.retain(|(n, _)| n != &name);
            }
        });
    }
    Ok(receiver)
}

/// `req.setTimeout(timeout[, callback])`：超时先调 callback 再发 `'timeout'`
/// （Go 经全局 `setTimeout` 排队，此处等价排宏任务）。
fn client_set_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let timeout = super::int_arg(args, 0, 0);
    let cb = args.get(1).copied().filter(|v| super::is_function(vm, *v));
    if timeout > 0 {
        let due_base = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
        if let Some(cb) = cb {
            schedule_task(vm, cb, timeout as u64);
        }
        let emit_fn = vm.alloc_native_fn("http:internal.timeoutEmit");
        state::push_timeout_target(receiver);
        // 与回调同队尾：显式追加到同一到期时刻，保证先 callback 后事件。
        vm.timer_counter += 1;
        let id = vm.timer_counter;
        vm.macro_tasks.push_back((
            id,
            due_base + timeout as u64,
            timeout as u64,
            Value::Object(emit_fn),
            false,
        ));
    }
    Ok(receiver)
}

/// `'timeout'` 事件标记任务：弹出队首请求对象并发事件。
fn timeout_emit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(target) = state::pop_timeout_target() {
        state::emit(vm, target, "timeout", &[])?;
    }
    Ok(Value::Undefined)
}

/// `req.setNoDelay([noDelay])` / `setSocketKeepAlive`：no-op 返回自身。
fn client_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `req.abort()`：置中止标记并触发 `'abort'`（仅首次）。
fn client_abort(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    let first = with_clients(|clients| {
        if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
            if !c.aborted {
                c.aborted = true;
                return true;
            }
        }
        false
    });
    if first {
        state::emit(vm, receiver, "abort", &[])?;
    }
    Ok(receiver)
}

/// `req.destroy([error])`：等同 abort 并始终发 `'abort'` + `'close'`。
fn client_destroy(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    with_clients(|clients| {
        if let Some(c) = clients.iter_mut().find(|c| c.obj == r.0) {
            c.aborted = true;
        }
    });
    state::emit(vm, receiver, "abort", &[])?;
    state::emit(vm, receiver, "close", &[])?;
    Ok(receiver)
}

/// `req.on(event, listener)`。
fn client_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, false);
        }
    }
    Ok(receiver)
}

/// `req.once(event, listener)`。
fn client_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, true);
        }
    }
    Ok(receiver)
}

/// `req.off(event, listener)` / `removeListener`。
fn client_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::remove_listener(r.0, &name, *cb);
        }
    }
    Ok(receiver)
}

// --- 泵（客户端半边） ------------------------------------------------------

/// 客户端事件泵：连接 → 写请求 → 读响应 → 派发回调与体事件。
pub(crate) fn pump_clients(vm: &mut Vm) -> Result<bool, VmError> {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let mut progressed = false;
    enum Delivery {
        Aborted(u32),
        DialError(Option<Value>, String),
        Response(Option<Value>, wire::ResponseHead, Vec<u8>),
    }
    let mut deliveries: Vec<Delivery> = Vec::new();
    with_clients(|clients| {
        for c in clients.iter_mut() {
            if c.stage == Stage::Done || !c.ended {
                continue;
            }
            match c.stage {
                Stage::Connecting => {
                    if c.aborted {
                        c.error_dispatched = true;
                        c.stage = Stage::Done;
                        deliveries.push(Delivery::Aborted(c.obj));
                        continue;
                    }
                    // Windows 上 localhost 常解析为 IPv6 ::1，而本 VM server
                    // 仅绑 IPv4（0.0.0.0）；显式映射 127.0.0.1 保证本机自连可用
                    let dial_host = if c.host.eq_ignore_ascii_case("localhost") {
                        "127.0.0.1".to_owned()
                    } else {
                        c.host.clone()
                    };
                    let addr = format!("{}:{}", dial_host, c.port);
                    // Agent keepAlive：优先复用池内存活连接，未命中才新建。
                    // TLS 请求不走连接池（会话不可复用；Node https 默认
                    // keepAlive=false 同语义——每请求新连接，但必须新建）。
                    let attempt = match super::state::pool_take(&addr) {
                        Some(stream) if !c.tls => Some(stream),
                        _ => addr
                            .to_socket_addrs()
                            .ok()
                            .and_then(|mut it| it.next())
                            .and_then(|a| {
                                TcpStream::connect_timeout(
                                    &a,
                                    std::time::Duration::from_millis(100),
                                )
                                .ok()
                            }),
                    };
                    match attempt {
                        Some(stream) => {
                            let _ = stream.set_nonblocking(true);
                            c.write_buf = wire::serialize_request(
                                &c.method,
                                &c.path,
                                &c.host_header,
                                &c.headers,
                                &c.body,
                            );
                            if c.tls {
                                // https：建立 rustls 客户端会话，握手由泵推进
                                let conn =
                                    crate::builtins::tls::make_client_config().and_then(|cfg| {
                                        crate::builtins::tls::server_name(&c.host).and_then(|sn| {
                                            rustls::ClientConnection::new(
                                                std::sync::Arc::new(cfg),
                                                sn,
                                            )
                                            .map_err(|e| e.to_string())
                                        })
                                    });
                                match conn {
                                    Ok(conn) => {
                                        c.stream = Some(stream);
                                        c.tls_conn = Some(conn);
                                        c.stage = Stage::Handshaking;
                                    }
                                    Err(_) => {
                                        c.error_dispatched = true;
                                        c.stage = Stage::Done;
                                        deliveries.push(Delivery::Aborted(c.obj));
                                    }
                                }
                            } else {
                                c.stream = Some(stream);
                                c.stage = Stage::Sending;
                            }
                        }
                        None => {
                            let callback = c.callback;
                            c.error_dispatched = true;
                            c.stage = Stage::Done;
                            if c.aborted {
                                deliveries.push(Delivery::Aborted(c.obj));
                            } else {
                                let msg = format!(
                                    "dial tcp {addr}: connect: No connection could be made because the target machine actively refused it"
                                );
                                deliveries.push(Delivery::DialError(callback, msg));
                            }
                        }
                    }
                }
                Stage::Handshaking => {
                    // TLS 握手推进：发 flight → 读对端 flight → process，
                    // 直至握手完成（rustls 内部缓冲跨轮保留）
                    let mut failed = false;
                    let (tls, stream) = (
                        c.tls_conn.as_mut().expect("Handshaking 必含会话"),
                        c.stream.as_mut().expect("Handshaking 必含 socket"),
                    );
                    loop {
                        match tls.write_tls(stream) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    if !failed {
                        loop {
                            match tls.read_tls(stream) {
                                Ok(0) => {
                                    failed = true;
                                    break;
                                }
                                Ok(_) => {
                                    if tls.process_new_packets().is_err() {
                                        failed = true;
                                        break;
                                    }
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => {
                                    failed = true;
                                    break;
                                }
                            }
                        }
                    }
                    if failed {
                        c.error_dispatched = true;
                        c.stage = Stage::Done;
                        deliveries.push(Delivery::Aborted(c.obj));
                    } else if !tls.is_handshaking() {
                        c.stage = Stage::Sending;
                    }
                }
                Stage::Sending => {
                    let mut done = false;
                    // TLS：明文请求体经会话 writer 编码，再 write_tls 直发
                    // （tls_conn 与 stream 同建于 Connecting 阶段）
                    if let Some(tls) = c.tls_conn.as_mut() {
                        use std::io::Write;
                        if let Some(stream) = c.stream.as_mut() {
                            while !c.write_buf.is_empty() {
                                match tls.writer().write(&c.write_buf) {
                                    Ok(n) => {
                                        c.write_buf.drain(..n);
                                    }
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => {
                                        c.eof = true;
                                        break;
                                    }
                                }
                            }
                            let _ = tls.writer().flush();
                            loop {
                                match tls.write_tls(stream) {
                                    Ok(0) => break,
                                    Ok(_) => {}
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => {
                                        c.eof = true;
                                        break;
                                    }
                                }
                            }
                        }
                        done = c.write_buf.is_empty();
                    } else if let Some(stream) = &mut c.stream {
                        match stream.write(&c.write_buf) {
                            Ok(n) => {
                                c.write_buf.drain(..n);
                                done = c.write_buf.is_empty();
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => {
                                c.eof = true;
                            }
                        }
                    }
                    if done && !c.eof {
                        c.stage = Stage::AwaitingResponse;
                    }
                }
                Stage::AwaitingResponse => {
                    if let Some(tls) = c.tls_conn.as_mut() {
                        // TLS：先 flush 残余密文，再 read_tls → process →
                        // reader 读明文进 read_buf
                        let _ = tls.writer().flush();
                        if let Some(stream) = &mut c.stream {
                            loop {
                                match tls.write_tls(stream) {
                                    Ok(0) => break,
                                    Ok(_) => {}
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                        let mut tcp_eof = false;
                        if let Some(stream) = &mut c.stream {
                            loop {
                                match tls.read_tls(stream) {
                                    Ok(0) => {
                                        tcp_eof = true;
                                        break;
                                    }
                                    Ok(_) => {
                                        if tls.process_new_packets().is_err() {
                                            tcp_eof = true;
                                            break;
                                        }
                                    }
                                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        break;
                                    }
                                    Err(_) => {
                                        tcp_eof = true;
                                        break;
                                    }
                                }
                            }
                        }
                        // 解密明文（TLS EOF 亦须读尽已解出数据）
                        loop {
                            let mut tmp = [0u8; 8192];
                            match tls.reader().read(&mut tmp) {
                                Ok(0) => {
                                    c.eof = true;
                                    break;
                                }
                                Ok(n) => c.read_buf.extend_from_slice(&tmp[..n]),
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => {
                                    // 会话异常/未收 close_notify 即断开：无更多明文
                                    if tcp_eof {
                                        c.eof = true;
                                    }
                                    break;
                                }
                            }
                        }
                    } else if let Some(stream) = &mut c.stream {
                        loop {
                            let mut tmp = [0u8; 8192];
                            match stream.read(&mut tmp) {
                                Ok(0) => {
                                    c.eof = true;
                                    break;
                                }
                                Ok(n) => c.read_buf.extend_from_slice(&tmp[..n]),
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => {
                                    c.eof = true;
                                    break;
                                }
                            }
                        }
                    }
                    if let Some((head, body, _eof_delimited)) =
                        wire::try_take_response(&mut c.read_buf, c.eof)
                    {
                        let callback = c.callback;
                        c.stage = Stage::Done;
                        // keepAlive：完整响应归还连接池（非 keep-alive 响应会被
                        // 对端关闭，pool_take 探活自动淘汰）；TLS 请求直接关闭
                        if !c.tls {
                            if let Some(stream) = c.stream.take() {
                                super::state::pool_put(&format!("{}:{}", c.host, c.port), stream);
                            }
                        }
                        deliveries.push(Delivery::Response(callback, head, body));
                    } else if c.eof && c.read_buf.is_empty() {
                        // 连接被对端关闭且无响应：socket hang up
                        c.error_dispatched = true;
                        c.stage = Stage::Done;
                        deliveries.push(Delivery::Aborted(c.obj));
                    }
                }
                Stage::Done => {}
            }
        }
        clients.retain(|c| c.stage != Stage::Done);
    });
    for d in deliveries {
        progressed = true;
        match d {
            Delivery::Aborted(obj) => {
                let err = vm.alloc_error_instance("socket hang up");
                let code = vm.alloc_string("ECONNRESET".to_owned());
                let _ = vm.set_property(Value::Object(err), "code", Value::Object(code));
                let target = Value::Object(aluka_core::ObjectRef(obj));
                state::emit(vm, target, "error", &[Value::Object(err)])?;
                state::emit(vm, target, "close", &[])?;
            }
            Delivery::DialError(callback, msg) => {
                if let Some(cb) = callback {
                    let err = vm.alloc_string(msg);
                    vm.invoke_callable(
                        cb,
                        Value::Undefined,
                        &[Value::Undefined, Value::Object(err)],
                    )?;
                }
            }
            Delivery::Response(callback, head, body) => {
                deliver_response(vm, callback, &head, &body)?;
            }
        }
    }
    Ok(progressed)
}

/// 交付响应：构造 `IncomingMessage`（statusCode/statusMessage/headers/trailers）
/// → 回调 → 微任务收口 → `'data'`(Buffer)/`'end'`。
fn deliver_response(
    vm: &mut Vm,
    callback: Option<Value>,
    head: &wire::ResponseHead,
    body: &[u8],
) -> Result<(), VmError> {
    let res_val = super::build_message_instance(vm, "", "", &head.headers);
    let _ = vm.set_property(res_val, "statusCode", Value::Number(head.status as f64));
    let sm = vm.alloc_string(head.status_message.clone());
    let _ = vm.set_property(res_val, "statusMessage", Value::Object(sm));
    let trailers = vm.alloc_ordinary();
    let _ = vm.set_property(res_val, "trailers", Value::Object(trailers));
    if let Some(cb) = callback {
        vm.invoke_callable(cb, Value::Undefined, &[res_val])?;
    }
    vm.drain_microtasks()?;
    if !body.is_empty() {
        let chunk = Value::Object(buffer::create_buffer_instance(vm, body.to_vec()));
        state::emit(vm, res_val, "data", &[chunk])?;
    }
    state::emit(vm, res_val, "end", &[])?;
    Ok(())
}
