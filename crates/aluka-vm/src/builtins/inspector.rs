//! `inspector` 内置模块（Phase 7）：Chrome DevTools Protocol 会话 API 面。
//!
//! 照实移植 Node.js 22 LTS 规范的
//! 纯解释器定位：无真实 CDP 通信，仅提供 API 面（存在性检测与轻量交互）：
//! - `open` / `close` / `waitForDebugger`：空实现（不报错）；
//! - `url()`：无活动 CDP 端点返回 undefined；
//! - `console`：log/info/warn/error/debug/trace 六个空函数；
//! - `Network`：6 个 CDP 事件广播函数；`NetworkResources`：`put` 方法面；
//! - `Session`：EventEmitter 实例，`connect` / `disconnect` /
//!   `connectToMainThread` 切换连接标记，`post(method[, ...][, callback])`
//!   未连接时同步抛 `ERR_INSPECTOR_NOT_CONNECTED`，已连接且带回调时以
//!   `(null, { method, status: "ok" })` 同步应答。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

// Session 实例连接标记表：实例句柄 → 是否已连接。
thread_local! {
    // SESSIONS：线程局部（堆句柄仅本线程 Vm 有效）。
    static SESSIONS: RefCell<Option<HashMap<u32, bool>>> = const { RefCell::new(None) };
}

/// `require("inspector")` / `require("node:inspector")`。
pub const MODULE: ModuleDef = ModuleDef {
    name: "inspector",
    build,
};

/// 会话连接标记访问。
fn session_connected<R>(id: u32, f: impl FnOnce(&mut bool) -> R) -> R {
    SESSIONS.with(|g| {
        let mut guard = g.borrow_mut();
        f(guard
            .get_or_insert_with(HashMap::new)
            .entry(id)
            .or_default())
    })
}

/// 当前接收者（Session 实例）句柄 id。
fn receiver_id() -> Option<u32> {
    match current_receiver().case() {
        ValueCase::Object(r) => Some(r.0),
        _ => None,
    }
}

/// 值是否为可调用函数。
fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
            if matches!(
                vm.heap.get(r.0 as usize),
                Some(HeapObject::Closure { .. })
                    | Some(HeapObject::NativeCtor { .. })
                    | Some(HeapObject::NativeFn { .. })
            )
    )
}

/// 把 events 模块的 EventEmitter 实例方法处理器复制到目标命名空间
/// （Session 借用 events 的监听器存储，方法行为与 EventEmitter 完全一致）。
fn copy_emitter_handlers(registry: &mut BuiltinRegistry, ns: &str) {
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
            registry.dispatch.insert(format!("{ns}.{method}"), handler);
        }
    }
}

/// 空实现方法（open/close/waitForDebugger/console/Network/put 等）。
fn no_op(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 管理面：open/close/url/waitForDebugger
    for (key, name) in [
        ("open", "inspector.open"),
        ("close", "inspector.close"),
        ("url", "inspector.url"),
        ("waitForDebugger", "inspector.waitForDebugger"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, key, Value::Object(fn_ref))?;
    }
    register_handler(registry, "inspector", "open", no_op);
    register_handler(registry, "inspector", "close", no_op);
    register_handler(registry, "inspector", "url", no_op);
    register_handler(registry, "inspector", "waitForDebugger", no_op);

    // inspector.console：CDP 映射的 Console API（空实现）
    let console = vm.alloc_ordinary();
    let console_ns = Value::Object(vm.alloc_string("inspector:console".to_owned()));
    let _ = vm.set_property(Value::Object(console), "_builtinNs", console_ns);
    for lvl in ["log", "info", "warn", "error", "debug", "trace"] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector.console.{lvl}"));
        let _ = vm.set_property(Value::Object(console), lvl, Value::Object(fn_ref));
        registry
            .dispatch
            .insert(format!("inspector.console.{lvl}"), no_op);
        register_handler(registry, "inspector:console", lvl, no_op);
    }
    set_module_prop(vm, obj, "console", Value::Object(console))?;

    // inspector.Network：CDP Network 域事件广播函数（Node 语义：均为函数）
    let network = vm.alloc_ordinary();
    for evt in [
        "dataReceived",
        "dataSent",
        "requestWillBeSent",
        "responseReceived",
        "loadingFinished",
        "loadingFailed",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector.Network.{evt}"));
        let _ = vm.set_property(Value::Object(network), evt, Value::Object(fn_ref));
        registry
            .dispatch
            .insert(format!("inspector.Network.{evt}"), no_op);
    }
    set_module_prop(vm, obj, "Network", Value::Object(network))?;

    // inspector.NetworkResources：资源追踪句柄（put 方法面）
    let network_resources = vm.alloc_ordinary();
    let put_ref = vm.alloc_native_fn("inspector.NetworkResources.put");
    let _ = vm.set_property(
        Value::Object(network_resources),
        "put",
        Value::Object(put_ref),
    );
    registry
        .dispatch
        .insert("inspector.NetworkResources.put".to_owned(), no_op);
    set_module_prop(
        vm,
        obj,
        "NetworkResources",
        Value::Object(network_resources),
    )?;

    // Session：EventEmitter 实例（借用 events 监听器存储）
    let session_ref = vm.alloc_native_fn("inspector.Session");
    set_module_prop(vm, obj, "Session", Value::Object(session_ref))?;
    register_handler(registry, "inspector", "Session", session_ctor);
    copy_emitter_handlers(registry, "inspector:session");
    for (method, handler) in [
        ("connect", session_connect as BuiltinHandler),
        ("disconnect", session_disconnect),
        ("connectToMainThread", session_connect_to_main_thread),
        ("post", session_post),
    ] {
        register_handler(registry, "inspector:session", method, handler);
    }

    Ok(obj)
}

/// `new inspector.Session()`（可不带 new）：EventEmitter 实例 + 会话方法面。
fn session_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let inst = crate::builtins::events::create_emitter_instance(vm);
    let ns = Value::Object(vm.alloc_string("inspector:session".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    for method in ["connect", "disconnect", "connectToMainThread", "post"] {
        let fn_ref = vm.alloc_native_fn(&format!("inspector:session.{method}"));
        let _ = vm.set_property(Value::Object(inst), method, Value::Object(fn_ref));
    }
    SESSIONS.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(inst.0, false);
    });
    Ok(Value::Object(inst))
}

/// `session.connect()`。
fn session_connect(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_id() {
        session_connected(id, |connected| *connected = true);
    }
    Ok(Value::Undefined)
}

/// `session.disconnect()`。
fn session_disconnect(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_id() {
        session_connected(id, |connected| *connected = false);
    }
    Ok(Value::Undefined)
}

/// `session.connectToMainThread()`。
fn session_connect_to_main_thread(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_id() {
        session_connected(id, |connected| *connected = true);
    }
    Ok(Value::Undefined)
}

/// `session.post(method[, params][, callback])`：未连接同步抛
/// `ERR_INSPECTOR_NOT_CONNECTED`；已连接且带回调时以 `(null, 结果对象)` 同步
/// 应答（结果对象含 `method` 与 `status: "ok"`）。
fn session_post(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let connected = session_connected(id, |connected| *connected);
    if !connected {
        let err = vm.alloc_error_instance("Session is not connected");
        let code = Value::Object(vm.alloc_string("ERR_INSPECTOR_NOT_CONNECTED".to_owned()));
        let _ = vm.set_property(Value::Object(err), "code", code);
        return Err(VmError::Thrown(Value::Object(err)));
    }
    if args.is_empty() {
        return Ok(Value::Undefined);
    }
    let method = vm.format_value(args[0]);
    let callback = args.iter().skip(1).find(|v| is_function(vm, **v)).copied();
    if let Some(cb) = callback {
        let res = vm.alloc_ordinary();
        let method_val = Value::Object(vm.alloc_string(method));
        let _ = vm.set_property(Value::Object(res), "method", method_val);
        let status_val = Value::Object(vm.alloc_string("ok".to_owned()));
        let _ = vm.set_property(Value::Object(res), "status", status_val);
        vm.invoke_callable(cb, Value::Undefined, &[Value::Null, Value::Object(res)])?;
    }
    Ok(Value::Undefined)
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = no_op;
        let _: crate::builtins::BuiltinHandler = session_ctor;
        let _: crate::builtins::BuiltinHandler = session_connect;
        let _: crate::builtins::BuiltinHandler = session_disconnect;
        let _: crate::builtins::BuiltinHandler = session_connect_to_main_thread;
        let _: crate::builtins::BuiltinHandler = session_post;
    }
}

#[cfg(test)]
mod probe_tests {
    use crate::Vm;
    use crate::value::Value;

    #[test]
    fn probe_session_construct() {
        let mut vm = Vm::new(0);
        vm.setup_cjs(std::path::Path::new("dummy.cjs"));
        let req = Value::Object(vm.alloc_string("node:inspector".to_owned()));
        let insp = vm.call_require(req).expect("require node:inspector");
        let session_ctor_val = vm.get_property(insp, "Session").expect("Session");
        println!("Session = {session_ctor_val:?}");
        let inst = vm.do_construct(session_ctor_val, &[]);
        match &inst {
            Ok(v) => {
                let conn = vm.get_property(*v, "connect").expect("get connect");
                println!("connect = {conn:?}");
                assert!(conn.is_object(), "new Session() 实例应带 connect 方法");
            }
            Err(e) => panic!("do_construct 失败: {e:?}"),
        }
    }
}
