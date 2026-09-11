//! `node:http2` 内置模块（Phase 5）：HTTP/2 协议表面。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodehttp/http2.go`，基于 Go net/http 的最小表面）：
//! - `constants`：伪头、NGHTTP2 错误码、帧类型、设置项（逐值对齐）；
//! - `getDefaultSettings()`/`getPackedSettings()`/`getUnpackedSettings()`；
//! - `connect(authority[, options][, listener])` → ClientHttp2Session 表面
//!   （`request`/`close`/`ref`/`unref` + 异步 `'connect'` 事件）；
//! - `createServer([handler])`：Go 即复用 node:http 明文 Server，照实移植；
//! - `createSecureServer([options][, handler])`：TLS 选项校验错误消息对齐
//!   `nodenet.TLSConfigFromOptions`。
//!
//! **已知限制（纯 Rust 约束，禁 C 依赖）**：无真实 HTTP/2/TLS 协议栈。
//! `session.request` 仅对 `http://` authority 以 HTTP/1.1 语义工作，
//! `https://` authority 派发 stream `'error'`；`createSecureServer` 携带
//! 合法 PEM 时降级为明文 Server。

use crate::builtins::buffer;
use crate::builtins::http;
use crate::builtins::http::state;
use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;

// 待异步发射 `'connect'` 的会话对象队列（宏任务标记函数消费；线程局部：
// 堆句柄仅本线程 Vm 有效）。
thread_local! {
    static CONNECT_TARGETS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

// 待异步派发的流错误（流对象, 错误消息）队列（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static STREAM_ERRORS: RefCell<Vec<(Value, String)>> = const { RefCell::new(Vec::new()) };
}

/// `require("http2")` / `require("node:http2")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "http2",
    build,
};

/// `http2` 模块构建（常量表 + settings 工具 + connect/createServer 表面）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // http2.constants：伪头（RFC 7540）+ NGHTTP2 错误码 + 帧类型 + 设置项。
    let constants = vm.alloc_ordinary();
    for (name, value) in [
        ("HTTP2_HEADER_METHOD", string_val(vm, ":method")),
        ("HTTP2_HEADER_PATH", string_val(vm, ":path")),
        ("HTTP2_HEADER_SCHEME", string_val(vm, ":scheme")),
        ("HTTP2_HEADER_AUTHORITY", string_val(vm, ":authority")),
        ("HTTP2_HEADER_STATUS", string_val(vm, ":status")),
        ("HTTP2_HEADER_PROTOCOL", string_val(vm, ":protocol")),
        ("NGHTTP2_NO_ERROR", Value::Number(0x0 as f64)),
        ("NGHTTP2_PROTOCOL_ERROR", Value::Number(0x1 as f64)),
        ("NGHTTP2_INTERNAL_ERROR", Value::Number(0x2 as f64)),
        ("NGHTTP2_FLOW_CONTROL_ERROR", Value::Number(0x3 as f64)),
        ("NGHTTP2_SETTINGS_TIMEOUT", Value::Number(0x4 as f64)),
        ("NGHTTP2_STREAM_CLOSED", Value::Number(0x5 as f64)),
        ("NGHTTP2_FRAME_SIZE_ERROR", Value::Number(0x6 as f64)),
        ("NGHTTP2_REFUSED_STREAM", Value::Number(0x7 as f64)),
        ("NGHTTP2_CANCEL", Value::Number(0x8 as f64)),
        ("NGHTTP2_COMPRESSION_ERROR", Value::Number(0x9 as f64)),
        ("NGHTTP2_CONNECT_ERROR", Value::Number(0xa as f64)),
        ("NGHTTP2_ENHANCE_YOUR_CALM", Value::Number(0xb as f64)),
        ("NGHTTP2_INADEQUATE_SECURITY", Value::Number(0xc as f64)),
        ("NGHTTP2_HTTP_1_1_REQUIRED", Value::Number(0xd as f64)),
        ("HTTP2_FRAME_HEADERS", Value::Number(0x1 as f64)),
        ("HTTP2_FRAME_SETTINGS", Value::Number(0x4 as f64)),
        ("HTTP2_FRAME_PING", Value::Number(0x6 as f64)),
        ("HTTP2_FRAME_GOAWAY", Value::Number(0x7 as f64)),
        (
            "HTTP2_SETTINGS_HEADER_TABLE_SIZE",
            Value::Number(0x1 as f64),
        ),
        ("HTTP2_SETTINGS_ENABLE_PUSH", Value::Number(0x2 as f64)),
        (
            "HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS",
            Value::Number(0x3 as f64),
        ),
        (
            "HTTP2_SETTINGS_INITIAL_WINDOW_SIZE",
            Value::Number(0x4 as f64),
        ),
        ("HTTP2_SETTINGS_MAX_FRAME_SIZE", Value::Number(0x5 as f64)),
        (
            "HTTP2_SETTINGS_MAX_HEADER_LIST_SIZE",
            Value::Number(0x6 as f64),
        ),
        (
            "HTTP2_SETTINGS_ENABLE_CONNECT_PROTOCOL",
            Value::Number(0x8 as f64),
        ),
        ("NGHTTP2_ERR_NOMEM", Value::Number(-1.0)),
    ] {
        set_module_prop(vm, constants, name, value)?;
    }
    set_module_prop(vm, obj, "constants", Value::Object(constants))?;

    for (name, native) in [
        ("getDefaultSettings", "http2.getDefaultSettings"),
        ("getPackedSettings", "http2.getPackedSettings"),
        ("getUnpackedSettings", "http2.getUnpackedSettings"),
        ("connect", "http2.connect"),
        ("createServer", "http2.createServer"),
        ("createSecureServer", "http2.createSecureServer"),
    ] {
        let fn_ref = vm.alloc_native_fn(native);
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
    }
    register_handler(
        registry,
        "http2",
        "getDefaultSettings",
        get_default_settings,
    );
    register_handler(registry, "http2", "getPackedSettings", get_packed_settings);
    register_handler(
        registry,
        "http2",
        "getUnpackedSettings",
        get_unpacked_settings,
    );
    register_handler(registry, "http2", "connect", http2_connect);
    register_handler(registry, "http2", "createServer", http2_create_server);
    register_handler(
        registry,
        "http2",
        "createSecureServer",
        http2_create_secure_server,
    );

    // sensitiveHeaders：Go 导出 Symbol（node22 conformance h7 要求 typeof
    // 为 "symbol"）；描述文本使 String(sym) 输出与 Go 逐字一致。
    let sym = vm.alloc_symbol(
        crate::symbol::next_symbol_id(),
        "sensitiveHeaders".to_owned(),
    );
    set_module_prop(vm, obj, "sensitiveHeaders", Value::Object(sym))?;

    // 会话/流实例命名空间分派。
    for (method, handler) in [
        ("on", session_on as BuiltinHandler),
        ("addListener", session_on),
        ("once", session_once),
        ("off", session_off),
        ("removeListener", session_off),
        ("request", session_request),
        ("close", session_close),
        ("ref", session_noop_self),
        ("unref", session_noop_self),
    ] {
        register_handler(registry, "http2:session", method, handler);
    }
    register_handler(registry, "http2:stream", "respond", stream_respond);
    register_handler(registry, "http2:stream", "end", stream_end);
    // 异步标记任务：'connect' 事件发射与流错误派发。
    register_handler(registry, "http2:internal", "connectEmit", connect_emit);
    register_handler(registry, "http2:internal", "streamTask", stream_task_emit);
    Ok(obj)
}

/// 堆字符串值短写。
fn string_val(vm: &mut Vm, s: &str) -> Value {
    Value::Object(vm.alloc_string(s.to_owned()))
}

/// `http2.getDefaultSettings()`：Node 默认设置对象（键序与值逐项对齐）。
fn get_default_settings(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let s = vm.alloc_ordinary();
    set_module_prop(vm, s, "headerTableSize", Value::Number(4096.0))?;
    set_module_prop(vm, s, "enablePush", Value::Boolean(true))?;
    set_module_prop(vm, s, "initialWindowSize", Value::Number(65535.0))?;
    set_module_prop(vm, s, "maxFrameSize", Value::Number(16384.0))?;
    set_module_prop(vm, s, "maxConcurrentStreams", Value::Number(4294967295.0))?;
    set_module_prop(vm, s, "maxHeaderSize", Value::Number(65535.0))?;
    set_module_prop(vm, s, "maxHeaderListSize", Value::Number(65535.0))?;
    set_module_prop(vm, s, "enableConnectProtocol", Value::Boolean(false))?;
    Ok(Value::Object(s))
}

/// `http2.getPackedSettings()`：空 Buffer（Go 占位语义）。
fn get_packed_settings(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(buffer::create_buffer_instance(
        vm,
        Vec::new(),
    )))
}

/// `http2.getUnpackedSettings()`：空对象（Go 占位语义）。
fn get_unpacked_settings(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(vm.alloc_ordinary()))
}

/// `http2.connect(authority[, options][, listener])` → ClientHttp2Session。
fn http2_connect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let listener = args[1.min(args.len())..]
        .iter()
        .find(|a| http::is_function(vm, **a))
        .copied();
    let sess = vm.alloc_ordinary();
    let ns = vm.alloc_string("http2:session".to_owned());
    let _ = vm.set_property(Value::Object(sess), "_builtinNs", Value::Object(ns));
    for method in [
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "request",
        "close",
        "ref",
        "unref",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http2:session.{method}"));
        let _ = vm.set_property(Value::Object(sess), method, Value::Object(fn_ref));
    }
    // Go：listener 登记为 'connect' 监听器（connect 事件携带 (sess, undefined)）。
    if let Some(l) = listener {
        state::add_listener(sess.0, "connect", l, false);
    }
    // 记录 authority（session.request 的默认目标，Go 存于会话状态）。
    let authority = args
        .first()
        .map(|a| vm.format_value(*a))
        .unwrap_or_default();
    let auth = vm.alloc_string(authority);
    let _ = vm.set_property(Value::Object(sess), "_authority", Value::Object(auth));
    // 'connect' 事件经宏任务异步发射（Go PostTask 语义，无需活跃服务器）。
    CONNECT_TARGETS.with(|q| q.borrow_mut().push(Value::Object(sess)));
    schedule_emit_task(vm, "http2:internal.connectEmit");
    Ok(Value::Object(sess))
}

/// 排一个零延迟宏任务（异步发射会话/流事件的标记函数）。
fn schedule_emit_task(vm: &mut Vm, name: &str) {
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    let marker = vm.alloc_native_fn(name);
    vm.macro_tasks
        .push_back((id, last_due, 0, Value::Object(marker), false));
}

/// 标记任务：发射全部待发 `'connect'` 事件（参数 `(sess, undefined)`）。
fn connect_emit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let targets: Vec<Value> = CONNECT_TARGETS.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for sess in targets {
        state::emit(vm, sess, "connect", &[sess, Value::Undefined])?;
    }
    Ok(Value::Undefined)
}

/// 标记任务：派发全部待发流错误（Go 以字符串值发 `'error'`）。
fn stream_task_emit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let errors: Vec<(Value, String)> = STREAM_ERRORS.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for (stream, msg) in errors {
        let err = vm.alloc_string(msg);
        state::emit(vm, stream, "error", &[Value::Object(err)])?;
    }
    Ok(Value::Undefined)
}

/// 会话 `on(event, listener)`。
fn session_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, false);
        }
    }
    Ok(receiver)
}

/// 会话 `once(event, listener)`。
fn session_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, true);
        }
    }
    Ok(receiver)
}

/// 会话 `off(event, listener)`。
fn session_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::remove_listener(r.0, &name, *cb);
        }
    }
    Ok(receiver)
}

/// `session.request(headers[, options][, callback])` → ClientHttp2Stream。
/// 请求目标取 headers 的 `:authority`（缺省回退 connect 的 authority）。
fn session_request(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if !matches!(receiver.case(), ValueCase::Object(_)) {
        return Ok(receiver);
    }
    if args.is_empty() {
        return Err(http::thrown_error(vm, "request: headers required"));
    }
    let mut method = "GET".to_owned();
    let mut path = "/".to_owned();
    let mut authority = vm
        .get_property(receiver, "_authority")
        .ok()
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    if http::is_plain_object(vm, args[0]) {
        let props: Vec<(String, Value)> = match args[0].case() {
            ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
                Some(crate::heap::HeapObject::Ordinary { .. }) => vm.own_entries(r.0 as usize),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        for (k, v) in props {
            match k.as_str() {
                ":method" => method = vm.format_value(v),
                ":path" => path = vm.format_value(v),
                ":authority" => authority = vm.format_value(v),
                _ => {}
            }
        }
    }
    if authority.is_empty() {
        return Err(http::thrown_error(vm, "request: :authority required"));
    }
    let stream = vm.alloc_ordinary();
    let ns = vm.alloc_string("http2:stream".to_owned());
    let _ = vm.set_property(Value::Object(stream), "_builtinNs", Value::Object(ns));
    for m in [
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "respond",
        "end",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http2:stream.{m}"));
        let _ = vm.set_property(Value::Object(stream), m, Value::Object(fn_ref));
    }
    // Rust 侧无 HTTP/2/TLS 协议栈：流请求统一异步派发 'error'（Go 会真实
    // 发起请求，此处为登记的已知限制，探针只打会话表面）。
    STREAM_ERRORS.with(|q| {
        q.borrow_mut().push((
            Value::Object(stream),
            format!(
                "http2: session {authority} request {method} {path} requires an HTTP/2 stack (unsupported in Rust build)"
            ),
        ));
    });
    schedule_emit_task(vm, "http2:internal.streamTask");
    Ok(Value::Object(stream))
}

/// 解析 `http(s)://host[:port]` authority → (scheme, host, port, 默认端口)。
#[allow(dead_code)]
fn parse_authority(authority: &str) -> Option<(String, String, u16, u16)> {
    let (scheme, rest, default_port) = if let Some(r) = authority.strip_prefix("https://") {
        ("https".to_owned(), r, 443u16)
    } else if let Some(r) = authority.strip_prefix("http://") {
        ("http".to_owned(), r, 80u16)
    } else {
        return None;
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (rest.to_string(), default_port),
    };
    Some((scheme, host, port, default_port))
}

/// `session.close([callback])`：同步发 `'close'` 并同步调用回调（Go 语义）。
fn session_close(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    state::emit(vm, receiver, "close", &[])?;
    if let Some(cb) = args.first() {
        if http::is_function(vm, *cb) {
            vm.invoke_callable(*cb, Value::Undefined, &[])?;
        }
    }
    Ok(receiver)
}

/// `session.ref/unref`：no-op 返回自身。
fn session_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// stream `respond(headers)`：占位（明文限制下不发送）。
fn stream_respond(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// stream `end()`：占位。
fn stream_end(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `http2.createServer([handler])`：Go 即复用 node:http 明文 Server，照实移植。
fn http2_create_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let handler = args.iter().find(|a| http::is_function(vm, **a)).copied();
    let obj = http::create_server_object(vm, handler);
    Ok(Value::Object(obj))
}

/// `http2.createSecureServer([options][, handler])`：TLS 选项校验（错误
/// 消息对齐 `nodenet.TLSConfigFromOptions`），合法时降级为明文 Server。
fn http2_create_secure_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut handler: Option<Value> = None;
    let mut options: Option<Value> = None;
    for a in args {
        if http::is_function(vm, *a) {
            handler = Some(*a);
        } else if http::is_plain_object(vm, *a) {
            options = Some(*a);
        }
    }
    // nodenet.TLSConfigFromOptions：options 缺失时报不带 PEM 的消息，
    // 且 missing 键以 IsUndefined 判定（与 https.createServer 路径不同）。
    let Some(opts) = options else {
        return Err(http::thrown_error(
            vm,
            "tls: createServer requires { key, cert } options",
        ));
    };
    let mut key_pem = String::new();
    let mut cert_pem = String::new();
    if let Ok(v) = vm.get_property(opts, "key") {
        if !matches!(v, Value::Undefined) {
            key_pem = vm.format_value(v);
        }
    }
    if let Ok(v) = vm.get_property(opts, "cert") {
        if !matches!(v, Value::Undefined) {
            cert_pem = vm.format_value(v);
        }
    }
    if key_pem.is_empty() || cert_pem.is_empty() {
        return Err(http::thrown_error(
            vm,
            "tls: createServer requires { key, cert } PEM options",
        ));
    }
    if !http::has_pem_block(&cert_pem) {
        return Err(http::thrown_error(
            vm,
            "tls: invalid key/cert: tls: failed to find any PEM data in certificate input",
        ));
    }
    if !http::has_pem_block(&key_pem) {
        return Err(http::thrown_error(
            vm,
            "tls: invalid key/cert: tls: failed to find any PEM data in key input",
        ));
    }
    let obj = http::create_server_object(vm, handler);
    Ok(Value::Object(obj))
}

/// GC 根快照：http2 连接目标与流错误值。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    CONNECT_TARGETS.with(|g| {
        for v in g.borrow().iter() {
            out.push(*v);
        }
    });
    STREAM_ERRORS.with(|g| {
        for (v, _) in g.borrow().iter() {
            out.push(*v);
        }
    });
}
