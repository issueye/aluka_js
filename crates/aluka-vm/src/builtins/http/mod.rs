//! `node:http` 内置模块（Phase 5）：HTTP 服务器与客户端。
//!
//! 语义以 Node.js 22 LTS 标准（Node.js 22 LTS 规范 +
//! `http_agent.go`）为唯一真理逐字对齐：
//! - `createServer`（Server：`listen`/`close` + `'request'`/`'connection'`/
//!   `'listening'`/`'error'` 事件）、`request`/`get`（客户端）、
//!   `Agent`/`globalAgent`、`IncomingMessage`、`ServerResponse`、
//!   `STATUS_CODES`/`METHODS` 常量与 `validateHeaderName/Value`；
//! - 底层用真实 TCP（`std::net`）手写 HTTP/1.1 报文解析与生成
//!   （见 [`wire`]），服务端响应补 `Date`/`Content-Length`/嗅探
//!   `Content-Type`（bodyless 状态码除外），客户端有 body 走 chunked；
//! - 事件驱动经 `vm.activate_event_source("http", pump)` 接入顶层事件循环，
//!   泵内非阻塞 accept/读写 socket，解析完整报文后派发 JS 回调。

mod client;
pub(crate) mod server;
pub(crate) mod state;
pub(crate) mod wire;

/// 创建 Server 实例（https / http2 模块复用 Go `newHTTPServerWithTLS` 路径）。
pub(crate) fn create_server_object(vm: &mut Vm, handler: Option<Value>) -> ObjectRef {
    server::create_server_object(vm, handler)
}

/// 创建带 TLS 配置的 Server 实例（`https.createServer` 真实握手）。
pub(crate) fn create_server_object_tls(
    vm: &mut Vm,
    handler: Option<Value>,
    tls: Option<std::sync::Arc<rustls::ServerConfig>>,
) -> ObjectRef {
    server::create_server_object_tls(vm, handler, tls)
}

/// PEM 块结构校验（https / http2 的证书选项错误消息共用）。
pub(crate) fn has_pem_block(text: &str) -> bool {
    let Some(begin) = text.find("-----BEGIN ") else {
        return false;
    };
    let after_begin = match text[begin..]
        .find(
            "-----
",
        )
        .or_else(|| {
            text[begin..].find(
                "-----
",
            )
        }) {
        Some(i) => begin + i + 6,
        None => return false,
    };
    text[after_begin..].contains("-----END ")
}

use crate::builtins::buffer;
use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("http")` / `require("node:http")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "http",
    build,
};

/// 顶层事件源泵：先服务器半边（accept/派发请求/finish 事件），
/// 后客户端半边（连接/写请求/读响应/交付回调）；轮末同步泵生命周期。
pub(crate) fn pump(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = server::pump_servers(vm)?;
    if client::pump_clients(vm)? {
        progressed = true;
    }
    // 请求全部完成且无 listening server 时自动停泵（纯客户端请求场景）
    sync_event_source(vm);
    Ok(progressed)
}

/// 泵活跃条件：任一 listening server 或存在已 end 未完成的客户端请求
/// （独立 https.request 也须推进拨号/握手/响应——M3.2 TLS 接线）。
pub(crate) fn sync_event_source(vm: &mut Vm) {
    let any_listening = state::read_servers(|servers| servers.iter().any(|s| s.listening));
    let any_client = state::with_clients(|clients| {
        clients
            .iter()
            .any(|c| c.ended && c.stage != state::Stage::Done)
    });
    if any_listening || any_client {
        vm.activate_event_source("http", pump);
    } else {
        vm.deactivate_event_source("http");
    }
}

/// `http` 模块构建：模块表面 + Agent 表面 + 实例命名空间分派。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 方法属性 + 模块级分派。
    for (name, native) in [
        ("createServer", "http.createServer"),
        ("request", "http.request"),
        ("get", "http.get"),
        ("validateHeaderName", "http.validateHeaderName"),
        ("validateHeaderValue", "http.validateHeaderValue"),
        ("IncomingMessage", "http.IncomingMessage"),
        ("ServerResponse", "http.ServerResponse"),
        ("Agent", "http.Agent"),
    ] {
        let fn_ref = vm.alloc_native_fn(native);
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
    }
    register_handler(registry, "http", "createServer", http_create_server);
    register_handler(registry, "http", "request", http_request);
    register_handler(registry, "http", "get", http_get);
    register_handler(registry, "http", "validateHeaderName", validate_noop);
    register_handler(registry, "http", "validateHeaderValue", validate_noop);
    register_handler(registry, "http", "IncomingMessage", incoming_message_ctor);
    register_handler(registry, "http", "ServerResponse", server_response_ctor);
    register_handler(registry, "http", "Agent", agent_ctor);

    // STATUS_CODES 常量表（Go 子集逐项对齐）。
    let codes = vm.alloc_ordinary();
    for (code, text) in [
        (200, "OK"),
        (201, "Created"),
        (202, "Accepted"),
        (301, "Moved Permanently"),
        (302, "Found"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (500, "Internal Server Error"),
        (502, "Bad Gateway"),
        (503, "Service Unavailable"),
    ] {
        let t = vm.alloc_string(text.to_owned());
        set_module_prop(vm, codes, &code.to_string(), Value::Object(t))?;
    }
    set_module_prop(vm, obj, "STATUS_CODES", Value::Object(codes))?;

    // METHODS：标准方法名数组（与 Node 一致，顺序固定）。
    let methods: Vec<Value> = [
        "ACL",
        "BIND",
        "CHECKOUT",
        "CONNECT",
        "COPY",
        "DELETE",
        "GET",
        "HEAD",
        "LINK",
        "LOCK",
        "M-SEARCH",
        "MERGE",
        "MKACTIVITY",
        "MKCALENDAR",
        "MKCOL",
        "MOVE",
        "NOTIFY",
        "OPTIONS",
        "PATCH",
        "POST",
        "PROPFIND",
        "PROPPATCH",
        "PURGE",
        "PUT",
        "REBIND",
        "REPORT",
        "SEARCH",
        "SOURCE",
        "SUBSCRIBE",
        "TRACE",
        "UNBIND",
        "UNLINK",
        "UNLOCK",
        "UNSUBSCRIBE",
    ]
    .iter()
    .map(|m| Value::Object(vm.alloc_string((*m).to_owned())))
    .collect();
    let methods_arr = vm.alloc_array(methods);
    set_module_prop(vm, obj, "METHODS", Value::Object(methods_arr))?;

    // IncomingMessage.prototype / ServerResponse.prototype（express 兼容表面）。
    set_ctor_prototype(vm, obj, "IncomingMessage")?;
    set_ctor_prototype(vm, obj, "ServerResponse")?;

    // globalAgent（Node 19+ 默认 keepAlive 语义）。
    let ga = build_global_agent(vm);
    set_module_prop(vm, obj, "globalAgent", Value::Object(ga))?;

    // 实例命名空间：server/response 在 server.rs，request 在 client.rs，
    // message（IncomingMessage）与 agent 在此注册。
    server::register_handlers(registry);
    client::register_handlers(registry);
    register_agent_handlers(registry);
    for (method, handler) in [
        ("on", message_on as BuiltinHandler),
        ("addListener", message_on),
        ("once", message_once),
        ("off", message_off),
        ("removeListener", message_off),
        ("resume", message_noop_self),
        ("pause", message_noop_self),
        ("destroy", message_noop_self),
        ("unpipe", message_noop_self),
    ] {
        register_handler(registry, "http:message", method, handler);
    }
    Ok(obj)
}

/// 为模块对象上的构造器属性补 `prototype`/`constructor` 环（Go
/// `newIncomingMessageCtor` 等的 prototype 表面）。
fn set_ctor_prototype(vm: &mut Vm, module_obj: ObjectRef, name: &str) -> Result<(), VmError> {
    let Ok(ctor) = vm.get_property(Value::Object(module_obj), name) else {
        return Ok(());
    };
    let proto = vm.alloc_ordinary();
    set_module_prop(vm, proto, "constructor", ctor)?;
    set_module_prop(vm, module_obj, "prototype", Value::Object(proto))?;
    Ok(())
}

// --- 模块级方法 -----------------------------------------------------------

/// `http.createServer([handler])`。
fn http_create_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let handler = args.iter().find(|a| is_function(vm, **a)).copied();
    let obj = server::create_server_object(vm, handler);
    Ok(Value::Object(obj))
}

/// `http.request(options[, callback])`。
fn http_request(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    client::create_request_object(vm, args, "http")
}

/// `http.get(options[, callback])`：创建请求并自动 `end()`（Go 同步调用 end）。
fn http_get(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let req = client::create_request_object(vm, args, "http")?;
    let end_fn = vm.get_property(req, "end")?;
    vm.invoke_callable(end_fn, req, &[])?;
    Ok(req)
}

/// `http.validateHeaderName/Value`：低层校验 no-op，返回 undefined（Go 同）。
fn validate_noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `new http.IncomingMessage()`：空消息对象（`method`/`url` 空串）。
fn incoming_message_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(build_message_instance(vm, "", "", &[]))
}

/// `new http.ServerResponse()`：未绑定连接的响应对象（statusCode 200）。
fn server_response_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let __s = vm.alloc_string("http:response".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(__s));
    let __s = vm.alloc_string("http:message".to_owned());
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
    Ok(Value::Object(obj))
}

/// `new http.Agent([options])`：Agent 表面对象（Go `registerHttpAgent`）。
fn agent_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut keep_alive = false;
    let mut keep_alive_msecs = 1000.0f64;
    let mut max_sockets = 0.0f64; // 0 → Infinity
    if let Some(opts) = args.first().and_then(|v| v.as_object()) {
        let read = |vm: &mut Vm, key: &str| vm.get_property(Value::Object(opts), key).ok();
        if let Some(v) = read(vm, "keepAlive") {
            if !matches!(v, Value::Undefined) {
                keep_alive = vm.truthy(v);
            }
        }
        if let Some(n) = read(vm, "keepAliveMsecs").and_then(|v| v.as_number()) {
            keep_alive_msecs = n;
        }
        if let Some(n) = read(vm, "maxSockets").and_then(|v| v.as_number()) {
            max_sockets = n;
        }
    }
    let agent = build_agent_instance(vm, keep_alive, keep_alive_msecs, max_sockets);
    Ok(agent)
}

/// 构建 Agent 实例对象（属性与实例方法，Go `registerHttpAgent` 逐项对齐）。
fn build_agent_instance(
    vm: &mut Vm,
    keep_alive: bool,
    keep_alive_msecs: f64,
    max_sockets: f64,
) -> Value {
    let agent = vm.alloc_ordinary();
    let ns = vm.alloc_string("http:agent".to_owned());
    let _ = vm.set_property(Value::Object(agent), "_builtinNs", Value::Object(ns));
    let _ = vm.set_property(
        Value::Object(agent),
        "keepAlive",
        Value::Boolean(keep_alive),
    );
    let _ = vm.set_property(
        Value::Object(agent),
        "keepAliveMsecs",
        Value::Number(keep_alive_msecs),
    );
    let max = if max_sockets > 0.0 {
        max_sockets
    } else {
        f64::INFINITY
    };
    let _ = vm.set_property(Value::Object(agent), "maxSockets", Value::Number(max));
    let _ = vm.set_property(Value::Object(agent), "maxFreeSockets", Value::Number(256.0));
    let sockets = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(agent), "sockets", Value::Object(sockets));
    let free_sockets = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(agent),
        "freeSockets",
        Value::Object(free_sockets),
    );
    let requests = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(agent), "requests", Value::Object(requests));
    for method in ["getName", "createConnection", "destroy"] {
        let fn_ref = vm.alloc_native_fn(&format!("http:agent.{method}"));
        let _ = vm.set_property(Value::Object(agent), method, Value::Object(fn_ref));
    }
    Value::Object(agent)
}

/// 注册 Agent 实例命名空间（`http:agent`）的分派处理器。
fn register_agent_handlers(registry: &mut BuiltinRegistry) {
    register_handler(registry, "http:agent", "getName", agent_get_name);
    register_handler(
        registry,
        "http:agent",
        "createConnection",
        agent_create_connection,
    );
    register_handler(registry, "http:agent", "destroy", agent_destroy);
}

/// `agent.getName()`：固定返回 `"http"`（Go 逐字对齐，https 复用同名实现）。
fn agent_get_name(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(alloc_static_str(_vm, "http")))
}

/// `agent.createConnection(...)`：API 表面（连接实际由泵管理）。
fn agent_create_connection(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `agent.destroy()`：返回 undefined（Go `CloseIdleConnections` 后返回 undefined）。
fn agent_destroy(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// 分配堆字符串（辅助短写）。
fn alloc_static_str(vm: &mut Vm, s: &str) -> ObjectRef {
    vm.alloc_string(s.to_owned())
}

/// 构建 `globalAgent` 表面对象（Node 19+ keepAlive:true）。
/// Go 版无 `maxSockets`/`getName`/`createConnection`，仅 `destroy` 方法。
fn build_global_agent(vm: &mut Vm) -> ObjectRef {
    let ga = vm.alloc_ordinary();
    let ns = alloc_static_str(vm, "http:agent");
    let _ = vm.set_property(Value::Object(ga), "_builtinNs", Value::Object(ns));
    let _ = vm.set_property(Value::Object(ga), "keepAlive", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(ga), "keepAliveMsecs", Value::Number(1000.0));
    let _ = vm.set_property(Value::Object(ga), "maxFreeSockets", Value::Number(256.0));
    let sockets = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(ga), "sockets", Value::Object(sockets));
    let free_sockets = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(ga),
        "freeSockets",
        Value::Object(free_sockets),
    );
    let requests = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(ga), "requests", Value::Object(requests));
    let destroy = vm.alloc_native_fn("http:agent.destroy");
    let _ = vm.set_property(Value::Object(ga), "destroy", Value::Object(destroy));
    ga
}

// --- 共享辅助（server/client/https/http2 复用） ----------------------------

/// 构造 `http.Agent` 实例（https 模块复用 Go `registerHttpAgent` 表面）。
pub(crate) fn create_agent_object(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    agent_ctor(vm, args)
}

/// 构建 `globalAgent`（https 模块复用）。
pub(crate) fn global_agent(vm: &mut Vm) -> ObjectRef {
    build_global_agent(vm)
}

/// 创建 `ClientRequest`（https 模块以 proto=https 复用客户端）。
pub(crate) fn create_request_object(
    vm: &mut Vm,
    args: &[Value],
    proto: &str,
) -> Result<Value, VmError> {
    client::create_request_object(vm, args, proto)
}

/// 构造带 `message` 的抛出错误（JS 侧 `e.message` 可读，Go builtin error 语义）。
pub(crate) fn thrown_error(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}

/// `nodebase.IntArg`：第 `i` 个参数取整数值，缺失/非数返回 `default`。
pub(crate) fn int_arg(args: &[Value], i: usize, default: i64) -> i64 {
    match args
        .get(i)
        .map(|v| v.case())
        .unwrap_or(ValueCase::Undefined)
    {
        ValueCase::Number(n) => n as i64,
        _ => default,
    }
}

/// 判断值是否为普通对象（堆 Ordinary；堆字符串虽为 Object 包装但不算）。
pub(crate) fn is_plain_object(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r) if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Ordinary { .. }))
    )
}

/// 判断值是否为可调用对象（闭包 / 原生函数 / 原生构造器）。
pub(crate) fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r) if matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
        )
    )
}

/// 头值 → 字符串列表：数组展开（跳过 undefined/null），其余单值。
pub(crate) fn header_values(vm: &mut Vm, v: Value) -> Vec<String> {
    if let Some(r) = v.as_object() {
        if let Some(HeapObject::Array { elements, .. }) = vm.heap.get(r.0 as usize) {
            return elements
                .iter()
                .filter(|e| !(e.is_undefined() || e.is_null()))
                .map(|e| vm.format_value(*e))
                .collect();
        }
    }
    if matches!(v, Value::Undefined | Value::Null) {
        return Vec::new();
    }
    vec![vm.format_value(v)]
}

/// chunk 参数 → 字节：Buffer 直接取字节，其余按字符串格式化后编码。
pub(crate) fn chunk_bytes(vm: &mut Vm, v: Value) -> Vec<u8> {
    if let Some(bytes) = buffer::extract_bytes(vm, v) {
        return bytes;
    }
    vm.format_value(v).into_bytes()
}

/// 构造 `IncomingMessage` 实例（请求 / 客户端响应通用）。
/// `headers` 为 JS 可见头（小写名 → 值列表），多值以 `", "` 连接。
pub(crate) fn build_message_instance(
    vm: &mut Vm,
    method: &str,
    url: &str,
    headers: &[(String, Vec<String>)],
) -> Value {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("http:message".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    let method_str = vm.alloc_string(method.to_owned());
    let _ = vm.set_property(Value::Object(obj), "method", Value::Object(method_str));
    let url_str = vm.alloc_string(url.to_owned());
    let _ = vm.set_property(Value::Object(obj), "url", Value::Object(url_str));
    let version = vm.alloc_string("1.1".to_owned());
    let _ = vm.set_property(Value::Object(obj), "httpVersion", Value::Object(version));
    let headers_obj = vm.alloc_ordinary();
    for (name, vals) in headers {
        let joined = vm.alloc_string(vals.join(", "));
        let _ = vm.set_property(Value::Object(headers_obj), name, Value::Object(joined));
    }
    let _ = vm.set_property(Value::Object(obj), "headers", Value::Object(headers_obj));
    for method_name in [
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "resume",
        "pause",
        "destroy",
        "unpipe",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("http:message.{method_name}"));
        let _ = vm.set_property(Value::Object(obj), method_name, Value::Object(fn_ref));
    }
    Value::Object(obj)
}

// --- IncomingMessage 实例事件/流方法 ---------------------------------------

/// `message.on(event, listener)`。
fn message_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, false);
        }
    }
    Ok(receiver)
}

/// `message.once(event, listener)`。
fn message_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::add_listener(r.0, &name, *cb, true);
        }
    }
    Ok(receiver)
}

/// `message.off(event, listener)` / `removeListener`。
fn message_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        if let (Some(event), Some(cb)) = (args.first(), args.get(1)) {
            let name = vm.format_value(*event);
            state::remove_listener(r.0, &name, *cb);
        }
    }
    Ok(receiver)
}

/// `message.resume/pause/destroy/unpipe`：aluka 消息一次性给出，全部 no-op。
fn message_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}
