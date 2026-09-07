//! `inspector/promises` 内置模块（Phase 7）：Promise 版 CDP 会话 API 面。
//!
//! 照实移植 Node.js 22 LTS 规范：
//! - 管理面与 `node:inspector` 一致（open/close/url/waitForDebugger/console/
//!   Network/NetworkResources，空实现）；
//! - `Session`：EventEmitter 实例；`connect` / `disconnect` /
//!   `connectToMainThread` 空实现；`post(method[, params])` 返回 Promise——
//!   Go 以 `reject(err 字符串)` 结算；`post` 使用 reject 解析器
//!   （`resolve: false`），随引擎 rejection 语义落地为真实拒绝；
//! - Node 22 中该模块仅导出 Session 类 + 同 inspector 的管理 API。
//!
//! 注：早期版本引擎只有 fulfill 通路，`post()` 曾以错误字符串 fulfill 结算；
//! 引擎补齐 rejection 后该偏离已消除（`await post()` 在两侧均走 catch 分支）。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, register_handler, set_module_prop,
};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("inspector/promises")` / `require("node:inspector/promises")`。
pub const MODULE: ModuleDef = ModuleDef {
    name: "inspector/promises",
    build,
};

/// 空实现方法（open/close/url/waitForDebugger/console/Network/put、
/// connect/disconnect/connectToMainThread）。
fn no_op(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `session.post(method[, params])` → Promise：无 V8，以未连接错误字符串结算。
fn session_post(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let msg = " inspector: not connected (pure-Go runtime, no V8)";
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, false);
    crate::builtins::timers::set_resolver_val(
        resolver.0,
        Value::Object(vm.alloc_string(msg.to_owned())),
    );
    vm.microtask_queue.push_back(crate::builtins::Job::Call(
        Value::Object(resolver),
        Value::Undefined,
    ));
    Ok(Value::Object(promise))
}

/// `new inspector/promises.Session()`（可不带 new）。
fn session_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let inst = crate::builtins::events::create_emitter_instance(vm);
    let ns = Value::Object(vm.alloc_string("inspector/promises:session".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    for method in ["connect", "disconnect", "connectToMainThread", "post"] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector/promises:session.{method}"));
        let _ = vm.set_property(Value::Object(inst), method, Value::Object(fn_ref));
    }
    Ok(Value::Object(inst))
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 管理面（与 node:inspector 一致；空实现）
    for (key, name) in [
        ("open", "inspector/promises.open"),
        ("close", "inspector/promises.close"),
        ("url", "inspector/promises.url"),
        ("waitForDebugger", "inspector/promises.waitForDebugger"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, key, Value::Object(fn_ref))?;
    }
    register_handler(registry, "inspector/promises", "open", no_op);
    register_handler(registry, "inspector/promises", "close", no_op);
    register_handler(registry, "inspector/promises", "url", no_op);
    register_handler(registry, "inspector/promises", "waitForDebugger", no_op);

    let console = vm.alloc_ordinary();
    let console_ns = Value::Object(vm.alloc_string("inspector/promises:console".to_owned()));
    let _ = vm.set_property(Value::Object(console), "_builtinNs", console_ns);
    for lvl in ["log", "info", "warn", "error", "debug", "trace"] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector/promises.console.{lvl}"));
        let _ = vm.set_property(Value::Object(console), lvl, Value::Object(fn_ref));
        registry
            .dispatch
            .insert(format!("inspector/promises.console.{lvl}"), no_op);
        register_handler(registry, "inspector/promises:console", lvl, no_op);
    }
    set_module_prop(vm, obj, "console", Value::Object(console))?;

    let network = vm.alloc_ordinary();
    for evt in [
        "dataReceived",
        "dataSent",
        "requestWillBeSent",
        "responseReceived",
        "loadingFinished",
        "loadingFailed",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector/promises.Network.{evt}"));
        let _ = vm.set_property(Value::Object(network), evt, Value::Object(fn_ref));
        registry
            .dispatch
            .insert(format!("inspector/promises.Network.{evt}"), no_op);
    }
    set_module_prop(vm, obj, "Network", Value::Object(network))?;

    let network_resources = vm.alloc_ordinary();
    let put_ref = vm.alloc_native_fn("inspector/promises.NetworkResources.put");
    let _ = vm.set_property(
        Value::Object(network_resources),
        "put",
        Value::Object(put_ref),
    );
    registry
        .dispatch
        .insert("inspector/promises.NetworkResources.put".to_owned(), no_op);
    set_module_prop(
        vm,
        obj,
        "NetworkResources",
        Value::Object(network_resources),
    )?;

    // Session（Promise 版）
    let session_ref = vm.alloc_native_fn("inspector/promises.Session");
    set_module_prop(vm, obj, "Session", Value::Object(session_ref))?;
    register_handler(registry, "inspector/promises", "Session", session_ctor);
    // 实例同时是 EventEmitter（借用 events 监听器存储）
    for method in [
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "removeAllListeners",
        "listenerCount",
        "setMaxListeners",
        "getMaxListeners",
        "prependListener",
        "prependOnceListener",
        "eventNames",
        "listeners",
        "rawListeners",
    ] {
        if let Some(handler) = registry.lookup(&format!("events:instance.{method}")) {
            registry
                .dispatch
                .insert(format!("inspector/promises:session.{method}"), handler);
        }
    }
    let session_methods: [(&str, BuiltinHandler); 4] = [
        ("connect", no_op),
        ("disconnect", no_op),
        ("connectToMainThread", no_op),
        ("post", session_post),
    ];
    for (method, handler) in session_methods {
        register_handler(registry, "inspector/promises:session", method, handler);
    }

    Ok(obj)
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = no_op;
        let _: crate::builtins::BuiltinHandler = session_ctor;
        let _: crate::builtins::BuiltinHandler = session_post;
    }
}
