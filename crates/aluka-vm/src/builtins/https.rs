//! `node:https` 内置模块（Phase 5）：HTTPS 服务器与客户端。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodehttp/https.go`）：
//! - `createServer([options][, handler])`：逐字复刻 options 解析与错误消息
//!   —— key/cert 缺失（或为空串）抛
//!   `https: createServer requires { key, cert } PEM options`；
//!   PEM 无效抛 `https: invalid key/cert: tls: failed to find any PEM data
//!   in certificate input`（与 Go `tls.X509KeyPair` 对应）；
//! - `request`/`get` 复用 `http` 客户端（proto=https）；
//! - `Agent`/`globalAgent` 复用 `http` 的实现；`Server` 构造器返回空对象。
//!
//! **已知限制（纯 Rust 约束，禁 C 依赖）**：不做真实 TLS 握手。携带合法
//! PEM 的 `createServer` 返回可用的明文 HTTP Server；`https.request` 的
//! 客户端同样无法协商 TLS。探针与测试仅覆盖选项校验错误、方法表面与
//! `Server` 构造等确定性路径。

use crate::builtins::http;
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("https")` / `require("node:https")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "https",
    build,
};

/// `https` 模块构建（Go `NewHTTPS`：Agent 表面 + createServer/request/get/Server）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for (name, native) in [
        ("createServer", "https.createServer"),
        ("request", "https.request"),
        ("get", "https.get"),
        ("Server", "https.Server"),
        ("Agent", "https.Agent"),
    ] {
        let fn_ref = vm.alloc_native_fn(native);
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
    }
    register_handler(registry, "https", "createServer", https_create_server);
    register_handler(registry, "https", "request", https_request);
    register_handler(registry, "https", "get", https_get);
    register_handler(registry, "https", "Server", https_server_ctor);
    register_handler(registry, "https", "Agent", https_agent_ctor);
    let ga = http::global_agent(vm);
    set_module_prop(vm, obj, "globalAgent", Value::Object(ga))?;
    Ok(obj)
}

/// `https.createServer([options][, handler])`：校验并构建 rustls 服务端
/// 配置后创建真实 TLS Server（M3.2 接线：accept 连接进入 rustls 会话）。
fn https_create_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut handler: Option<Value> = None;
    let mut options: Option<Value> = None;
    for a in args {
        if http::is_function(vm, *a) {
            handler = Some(*a);
        } else if http::is_plain_object(vm, *a) {
            options = Some(*a);
        }
    }
    // Go 逐字对齐：Get("key")/Get("cert") 对缺失键返回 undefined，
    // 其 String() 为 "undefined"（非空），因此仅显式空串触发 requires 错误。
    let (mut key_pem, mut cert_pem) = (String::new(), String::new());
    if let Some(opts) = options {
        if let Ok(v) = vm.get_property(opts, "key") {
            key_pem = vm.format_value(v);
        }
        if let Ok(v) = vm.get_property(opts, "cert") {
            cert_pem = vm.format_value(v);
        }
    }
    if key_pem.is_empty() || cert_pem.is_empty() {
        return Err(http::thrown_error(
            vm,
            "https: createServer requires { key, cert } PEM options",
        ));
    }
    if let Err(msg) = validate_pem_pair(&cert_pem, &key_pem) {
        return Err(http::thrown_error(
            vm,
            &format!("https: invalid key/cert: {msg}"),
        ));
    }
    // M3.2：真实 rustls 服务端配置（含 http/1.1 ALPN）
    let tls_config = match crate::builtins::tls::make_server_config(&key_pem, &cert_pem) {
        Ok(cfg) => std::sync::Arc::new(cfg),
        Err(msg) => {
            return Err(http::thrown_error(
                vm,
                &format!("https: invalid key/cert: {msg}"),
            ));
        }
    };
    let obj = http::create_server_object_tls(vm, handler, Some(tls_config));
    Ok(Value::Object(obj))
}

/// `https.request(options[, callback])`：复用 http 客户端（proto=https）。
fn https_request(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    http::create_request_object(vm, args, "https")
}

/// `https.get(options[, callback])`：创建请求并自动 `end()`。
fn https_get(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let req = http::create_request_object(vm, args, "https")?;
    let end_fn = vm.get_property(req, "end")?;
    vm.invoke_callable(end_fn, req, &[])?;
    Ok(req)
}

/// `new https.Server()`：返回空对象（Go 表面）。
fn https_server_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(vm.alloc_ordinary()))
}

/// `new https.Agent(...)`：复用 http 的 Agent 表面。
fn https_agent_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    http::create_agent_object(vm, args)
}

/// PEM 证书/私钥对校验（Go `tls.X509KeyPair` 错误消息对齐的子集）：
/// cert 缺 PEM 块 → `tls: failed to find any PEM data in certificate input`；
/// key 缺 PEM 块 → `tls: failed to find any PEM data in key input`。
fn validate_pem_pair(cert_pem: &str, key_pem: &str) -> Result<(), String> {
    if !has_pem_block(cert_pem) {
        return Err("tls: failed to find any PEM data in certificate input".to_owned());
    }
    if !has_pem_block(key_pem) {
        return Err("tls: failed to find any PEM data in key input".to_owned());
    }
    Ok(())
}

/// 文本是否含完整的 `-----BEGIN`/`-----END` PEM 块结构。
fn has_pem_block(text: &str) -> bool {
    let begin = match text.find("-----BEGIN ") {
        Some(i) => i,
        None => return false,
    };
    let after_begin = match text[begin..]
        .find("-----\n")
        .or_else(|| text[begin..].find("-----\r\n"))
    {
        Some(i) => begin + i + 6,
        None => return false,
    };
    text[after_begin..].contains("-----END ")
}
