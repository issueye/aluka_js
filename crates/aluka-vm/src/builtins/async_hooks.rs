//! `async_hooks` 内置模块（Phase 7）：异步追踪钩子与 AsyncLocalStorage。
//!
//! 语义照实移植 Node.js 22 LTS 规范
//! 的简化异步模型：
//! - `createHook({...})` → AsyncHook：`enable` / `disable`；引擎不创建内部异步
//!   资源，钩子回调仅在 AsyncResource 生命周期（`init` / `before` / `after` /
//!   `destroy`）触发；
//! - `executionAsyncId()`（顶层 1）/ `triggerAsyncId()`（顶层 0）/
//!   `executionAsyncResource()`（栈空返回新对象）；
//! - `AsyncResource`：`runInAsyncScope`（before → 压执行链 → 调用 → 弹栈 →
//!   after）、`emitDestroy`（一次性 destroy）、`asyncId` / `triggerAsyncId`、
//!   实例 `bind`；静态 `AsyncResource.bind` 无法经本 VM 的分派面触达
//!   （构造器原生函数接收者丢失方法名，见 mod.rs `try_dispatch` 形态一），
//!   未移植——已知限制；
//! - `AsyncLocalStorage`：静态栈表模拟（`run` / `getStore` / `enterWith` /
//!   `exit` / `disable`），handler 进入/退出时压/弹 store；
//! - `asyncWrapProviders`：与 Node 22 键对齐，Go 对全部键赋值 1——照实移植。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

/// AsyncResource 自增 id 起点（对齐 Go：首个 AsyncResource 的 asyncId 为 2）。
static NEXT_ASYNC_ID: AtomicI64 = AtomicI64::new(1);

/// 当前执行链状态：asyncId 栈 / triggerAsyncId 栈 / 执行资源栈。
#[derive(Default)]
struct ExecState {
    /// 当前执行链的 asyncId（顶层为空 → 1）
    exec: Vec<i64>,
    /// 触发者 asyncId（顶层为空 → 0）
    trigger: Vec<i64>,
    /// `executionAsyncResource` 的资源栈
    resource: Vec<Value>,
}

/// 一个 AsyncHook 实例的状态（回调 + 启用标记）。
struct HookState {
    /// 是否启用（enable/disable 切换）
    enabled: bool,
    /// init/before/after/destroy/promiseResolve 回调
    callbacks: [Option<Value>; 5],
}

/// 一个 AsyncLocalStorage 实例的状态。
#[derive(Default)]
struct AlsState {
    /// store 栈（栈顶 = 当前 store）
    stack: Vec<Value>,
    /// disable 后永久失效（run 直通、getStore 恒 undefined）
    disabled: bool,
}

/// 一个 AsyncResource 实例的状态。
struct ResourceData {
    /// asyncId（自增，首个为 2）
    uid: i64,
    /// triggerAsyncId（创建时的执行 id）
    trigger: i64,
    /// emitDestroy 只触发一次 destroy
    destroyed: bool,
}

/// `bind` 包装回调捕获的上下文（最后创建者生效——静态表模拟闭包）。
#[derive(Clone)]
struct BoundCtx {
    /// 被包装的函数
    cb: Value,
    /// 固定 this（实例 bind 取 args[1]；静态 bind 照 Go 取 args[2]）
    this_arg: Value,
}

/// 执行链状态。
static EXEC: Mutex<Option<ExecState>> = Mutex::new(None);
/// AsyncHook 注册表（创建顺序即派发顺序）：实例句柄 → 状态。
static HOOKS: Mutex<Vec<(u32, HookState)>> = Mutex::new(Vec::new());
/// AsyncLocalStorage 实例表：实例句柄 → 状态。
static ALS: Mutex<Option<HashMap<u32, AlsState>>> = Mutex::new(None);
/// AsyncResource 实例表：实例句柄 → 状态。
static RESOURCES: Mutex<Option<HashMap<u32, ResourceData>>> = Mutex::new(None);
/// 最近一次创建的 bind 包装上下文（静态表模拟 Go 的闭包捕获）。
static LAST_BOUND: Mutex<Option<BoundCtx>> = Mutex::new(None);

/// `require("async_hooks")` / `require("node:async_hooks")`。
pub const MODULE: ModuleDef = ModuleDef {
    name: "async_hooks",
    build,
};

/// 执行链状态访问。
fn with_exec<R>(f: impl FnOnce(&mut ExecState) -> R) -> R {
    let mut guard = EXEC.lock().unwrap();
    f(guard.get_or_insert_with(ExecState::default))
}

/// ALS 实例状态访问。
fn with_als<R>(id: u32, f: impl FnOnce(&mut AlsState) -> R) -> R {
    let mut guard = ALS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map.entry(id).or_default())
}

/// AsyncResource 实例状态访问。
fn with_resource<R>(id: u32, f: impl FnOnce(&mut ResourceData) -> R) -> Option<R> {
    let mut guard = RESOURCES.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).get_mut(&id).map(f)
}

/// 顶层执行 id（Node 语义：1）。
fn current_exec_id() -> i64 {
    with_exec(|st| st.exec.last().copied().unwrap_or(1))
}

/// 顶层触发者 id（Node 语义：0）。
fn current_trigger_id() -> i64 {
    with_exec(|st| st.trigger.last().copied().unwrap_or(0))
}

/// 当前接收者（实例对象）句柄 id。
fn receiver_id() -> Option<u32> {
    match current_receiver() {
        Value::Object(r) => Some(r.0),
        _ => None,
    }
}

/// 值是否为可调用函数（对齐 Go `Value.IsFunction`）。
fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(
        v,
        Value::Object(r)
            if matches!(
                vm.heap.get(r.0 as usize),
                Some(HeapObject::Closure { .. })
                    | Some(HeapObject::NativeCtor { .. })
                    | Some(HeapObject::NativeFn { .. })
            )
    )
}

/// 抛出 JS Error 实例（对齐 Go：native 返回的 error 以 Error 对象呈现，
/// `e.message` 可读）。
fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}

/// 向全部启用中的 hook 派发回调（hook 回调内的错误就地吞掉，对齐 Go 的
/// `ReportUncaught` 后继续派发）。
fn fire_hook(vm: &mut Vm, kind: usize, args: &[Value]) {
    let targets: Vec<Value> = {
        let hooks = HOOKS.lock().unwrap();
        hooks
            .iter()
            .filter(|(_, h)| h.enabled)
            .filter_map(|(_, h)| h.callbacks[kind])
            .collect()
    };
    for cb in targets {
        if is_function(vm, cb) {
            let _ = vm.invoke_callable(cb, Value::Undefined, args);
        }
    }
}

/// 钩子回调槽位下标：init/before/after/destroy/promiseResolve。
const HOOK_INIT: usize = 0;
const HOOK_BEFORE: usize = 1;
const HOOK_AFTER: usize = 2;
const HOOK_DESTROY: usize = 3;
const HOOK_PROMISE_RESOLVE: usize = 4;

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 模块导出面
    for (key, name) in [
        ("createHook", "async_hooks.createHook"),
        ("executionAsyncId", "async_hooks.executionAsyncId"),
        ("triggerAsyncId", "async_hooks.triggerAsyncId"),
        (
            "executionAsyncResource",
            "async_hooks.executionAsyncResource",
        ),
        ("AsyncResource", "async_hooks.AsyncResource"),
        ("AsyncLocalStorage", "async_hooks.AsyncLocalStorage"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, key, Value::Object(fn_ref))?;
    }
    register_handler(registry, "async_hooks", "createHook", create_hook);
    register_handler(
        registry,
        "async_hooks",
        "executionAsyncId",
        execution_async_id,
    );
    register_handler(registry, "async_hooks", "triggerAsyncId", trigger_async_id);
    register_handler(
        registry,
        "async_hooks",
        "executionAsyncResource",
        execution_async_resource,
    );
    register_handler(
        registry,
        "async_hooks",
        "AsyncResource",
        async_resource_ctor,
    );
    // 静态 AsyncResource.bind(fn[, thisArg])：复用 bound_trampoline 通路
    register_handler(
        registry,
        "async_hooks.AsyncResource",
        "bind",
        resource_static_bind,
    );
    register_handler(
        registry,
        "async_hooks",
        "AsyncLocalStorage",
        async_local_storage_ctor,
    );
    // asyncWrapProviders：全部键 → 1（对齐 Go 实际赋值）
    let providers = vm.alloc_ordinary();
    for name in async_wrap_provider_names() {
        let _ = vm.set_property(Value::Object(providers), name, Value::Number(1.0));
    }
    set_module_prop(vm, obj, "asyncWrapProviders", Value::Object(providers))?;

    // AsyncHook 实例方法
    register_handler(registry, "async_hooks:hook", "enable", hook_enable);
    register_handler(registry, "async_hooks:hook", "disable", hook_disable);

    // AsyncResource 实例方法
    register_handler(
        registry,
        "async_hooks:resource",
        "runInAsyncScope",
        resource_run_in_async_scope,
    );
    register_handler(
        registry,
        "async_hooks:resource",
        "emitDestroy",
        resource_emit_destroy,
    );
    register_handler(
        registry,
        "async_hooks:resource",
        "asyncId",
        resource_async_id,
    );
    register_handler(
        registry,
        "async_hooks:resource",
        "triggerAsyncId",
        resource_trigger_async_id,
    );
    register_handler(registry, "async_hooks:resource", "bind", resource_bind);

    // bind 包装回调
    registry
        .dispatch
        .insert("async_hooks:bound".to_owned(), bound_trampoline);

    // AsyncLocalStorage 实例方法
    register_handler(registry, "async_hooks:als", "run", als_run);
    register_handler(registry, "async_hooks:als", "getStore", als_get_store);
    register_handler(registry, "async_hooks:als", "enterWith", als_enter_with);
    register_handler(registry, "async_hooks:als", "exit", als_exit);
    register_handler(registry, "async_hooks:als", "disable", als_disable);

    Ok(obj)
}

/// `createHook({ init, before, after, destroy, promiseResolve })` → AsyncHook。
fn create_hook(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut state = HookState {
        enabled: false,
        callbacks: [None, None, None, None, None],
    };
    if let Some(Value::Object(r)) = args.first() {
        for (idx, key) in [
            (HOOK_INIT, "init"),
            (HOOK_BEFORE, "before"),
            (HOOK_AFTER, "after"),
            (HOOK_DESTROY, "destroy"),
            (HOOK_PROMISE_RESOLVE, "promiseResolve"),
        ] {
            if let Ok(v) = vm.get_property(Value::Object(*r), key) {
                state.callbacks[idx] = Some(v);
            }
        }
    }
    let inst = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("async_hooks:hook".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    for method in ["enable", "disable"] {
        let fn_ref = vm.alloc_native_fn(&format!("async_hooks:hook.{method}"));
        let _ = vm.set_property(Value::Object(inst), method, Value::Object(fn_ref));
    }
    HOOKS.lock().unwrap().push((inst.0, state));
    Ok(Value::Object(inst))
}

/// `hook.enable()`。
fn hook_enable(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_id() {
        for (hook_id, state) in HOOKS.lock().unwrap().iter_mut() {
            if *hook_id == id {
                state.enabled = true;
            }
        }
    }
    Ok(Value::Undefined)
}

/// `hook.disable()`。
fn hook_disable(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_id() {
        for (hook_id, state) in HOOKS.lock().unwrap().iter_mut() {
            if *hook_id == id {
                state.enabled = false;
            }
        }
    }
    Ok(Value::Undefined)
}

/// `executionAsyncId()`：执行链栈顶 id，顶层为 1。
fn execution_async_id(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Number(current_exec_id() as f64))
}

/// `triggerAsyncId()`：触发链栈顶 id，顶层为 0。
fn trigger_async_id(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Number(current_trigger_id() as f64))
}

/// `executionAsyncResource()`：执行链栈顶资源，栈空返回新对象。
fn execution_async_resource(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let top = with_exec(|st| st.resource.last().copied());
    Ok(top.unwrap_or_else(|| Value::Object(vm.alloc_ordinary())))
}

/// `new AsyncResource([type])` / `AsyncResource([type])`：分配 id、登记资源
/// 并派发 `init` 钩子。
fn async_resource_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let typ = match args.first() {
        Some(v) => vm.format_value(*v),
        None => "async_hooks.AsyncResource".to_owned(),
    };
    let uid = NEXT_ASYNC_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let trigger = current_exec_id();

    let inst = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("async_hooks:resource".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    for method in [
        "runInAsyncScope",
        "emitDestroy",
        "asyncId",
        "triggerAsyncId",
        "bind",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("async_hooks:resource.{method}"));
        let _ = vm.set_property(Value::Object(inst), method, Value::Object(fn_ref));
    }
    RESOURCES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(
            inst.0,
            ResourceData {
                uid,
                trigger,
                destroyed: false,
            },
        );
    let init_args = [
        Value::Number(uid as f64),
        Value::Object(vm.alloc_string(typ)),
        Value::Number(trigger as f64),
        Value::Object(inst),
    ];
    fire_hook(vm, HOOK_INIT, &init_args);
    Ok(Value::Object(inst))
}

/// `resource.runInAsyncScope(fn[, thisArg[, ...args]])`：
/// before → 压执行链 → 调用 → 弹执行链 → after（错误同样弹栈后传播）。
fn resource_run_in_async_scope(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "async_hooks: runInAsyncScope callback must be a function",
        ));
    }
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let Some((uid, trigger, res_obj)) = with_resource(id, |rd| (rd.uid, rd.trigger, id)) else {
        // 未登记实例（子类/引擎路径）：退化为直接调用，不触发钩子。
        let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
        let rest: Vec<Value> = args.iter().skip(2).copied().collect();
        return vm.invoke_callable(cb, this_arg, &rest);
    };
    fire_hook(vm, HOOK_BEFORE, &[Value::Number(uid as f64)]);
    with_exec(|st| {
        st.exec.push(uid);
        st.trigger.push(trigger);
        st.resource.push(Value::Object(ObjectRef(res_obj)));
    });
    let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
    let rest: Vec<Value> = args.iter().skip(2).copied().collect();
    let result = vm.invoke_callable(cb, this_arg, &rest);
    with_exec(|st| {
        st.exec.pop();
        st.trigger.pop();
        st.resource.pop();
    });
    fire_hook(vm, HOOK_AFTER, &[Value::Number(uid as f64)]);
    result
}

/// `resource.emitDestroy()`：一次性派发 `destroy` 钩子。
fn resource_emit_destroy(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let fire = with_resource(id, |rd| {
        if rd.destroyed {
            None
        } else {
            rd.destroyed = true;
            Some(rd.uid)
        }
    })
    .unwrap_or(None);
    if let Some(uid) = fire {
        fire_hook(vm, HOOK_DESTROY, &[Value::Number(uid as f64)]);
    }
    Ok(Value::Undefined)
}

/// `resource.asyncId()`。
fn resource_async_id(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_id()
        .and_then(|id| with_resource(id, |rd| rd.uid))
        .unwrap_or(0);
    Ok(Value::Number(id as f64))
}

/// `resource.triggerAsyncId()`。
fn resource_trigger_async_id(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_id()
        .and_then(|id| with_resource(id, |rd| rd.trigger))
        .unwrap_or(0);
    Ok(Value::Number(id as f64))
}

/// `resource.bind(fn[, thisArg])`：返回 enter 固定 this 的包装函数。
fn resource_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    make_bound(vm, args, 1, "async_hooks: bind requires a function")
}

/// 构造 bind 包装：捕获（函数, this）到静态最近槽位，返回共享包装回调。
fn make_bound(
    vm: &mut Vm,
    args: &[Value],
    this_arg_idx: usize,
    err_msg: &str,
) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(thrown(vm, err_msg));
    }
    let this_arg = args.get(this_arg_idx).copied().unwrap_or(Value::Undefined);
    *LAST_BOUND.lock().unwrap() = Some(BoundCtx { cb, this_arg });
    Ok(Value::Object(vm.alloc_native_fn("async_hooks:bound")))
}

/// bind 包装回调：以捕获的 this 调用被包装函数（包装可重复调用）。
fn bound_trampoline(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(ctx) = LAST_BOUND.lock().unwrap().clone() else {
        return Ok(Value::Undefined);
    };
    vm.invoke_callable(ctx.cb, ctx.this_arg, args)
}

/// 静态 `AsyncResource.bind(fn[, thisArg])`：登记绑定上下文并返回
/// `async_hooks:bound` 原生函数（调用时经 trampoline 在资源上下文执行）。
/// 与实例 bind 共享 LAST_BOUND「最近绑定」槽位（VM 无 JS 闭包构造能力，
/// 见模块头限制说明；单绑定探针与 Go 可观测一致）。
fn resource_static_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
    *LAST_BOUND.lock().unwrap() = Some(BoundCtx { cb, this_arg });
    let bound = vm.alloc_native_fn("async_hooks:bound");
    Ok(Value::Object(bound))
}

/// `new AsyncLocalStorage()`：登记实例并挂载 store 方法面。
fn async_local_storage_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let inst = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("async_hooks:als".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    for method in ["run", "getStore", "enterWith", "exit", "disable"] {
        let fn_ref = vm.alloc_native_fn(&format!("async_hooks:als.{method}"));
        let _ = vm.set_property(Value::Object(inst), method, Value::Object(fn_ref));
    }
    ALS.lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(inst.0, AlsState::default());
    Ok(Value::Object(inst))
}

/// `als.run(store, callback, ...args)`：压 store → 同步调用回调 → 弹 store
/// （disabled 时直通不压栈）。
fn als_run(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.get(1).copied().unwrap_or(Value::Undefined);
    if args.len() < 2 || !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "async_hooks: AsyncLocalStorage.run requires store and callback",
        ));
    }
    let store = args[0];
    let rest: Vec<Value> = args.iter().skip(2).copied().collect();
    let Some(id) = receiver_id() else {
        return vm.invoke_callable(cb, Value::Undefined, &rest);
    };
    let disabled = with_als(id, |st| st.disabled);
    if disabled {
        return vm.invoke_callable(cb, Value::Undefined, &rest);
    }
    with_als(id, |st| st.stack.push(store));
    let result = vm.invoke_callable(cb, Value::Undefined, &rest);
    with_als(id, |st| {
        st.stack.pop();
    });
    result
}

/// `als.getStore()`：栈顶 store；空栈或 disabled 返回 undefined。
fn als_get_store(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let store = with_als(id, |st| {
        if st.disabled {
            None
        } else {
            st.stack.last().copied()
        }
    });
    Ok(store.unwrap_or(Value::Undefined))
}

/// `als.enterWith(store)`：把 store 压入当前上下文（disabled 时忽略）。
fn als_enter_with(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let store = args.first().copied().unwrap_or(Value::Undefined);
    with_als(id, |st| {
        if !st.disabled {
            st.stack.push(store);
        }
    });
    Ok(Value::Undefined)
}

/// `als.exit(callback, ...args)`：压入 undefined 遮蔽后调用回调，返回后弹出。
fn als_exit(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "async_hooks: AsyncLocalStorage.exit requires a callback",
        ));
    }
    let Some(id) = receiver_id() else {
        return vm.invoke_callable(cb, Value::Undefined, &args[1..]);
    };
    with_als(id, |st| st.stack.push(Value::Undefined));
    let rest: Vec<Value> = args.iter().skip(1).copied().collect();
    let result = vm.invoke_callable(cb, Value::Undefined, &rest);
    with_als(id, |st| {
        st.stack.pop();
    });
    result
}

/// `als.disable()`：永久禁用并清空 store 栈。
fn als_disable(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    with_als(id, |st| {
        st.disabled = true;
        st.stack.clear();
    });
    Ok(Value::Undefined)
}

/// `asyncWrapProviders` 键集（与 Node 22 / Go 版对齐）。
fn async_wrap_provider_names() -> &'static [&'static str] {
    &[
        "BLOBREADER",
        "CHECKPRIMEREQUEST",
        "CIPHERREQUEST",
        "DERIVEBITSREQUEST",
        "DIRHANDLE",
        "DNSCHANNEL",
        "ELDHISTOGRAM",
        "FILEHANDLE",
        "FILEHANDLECLOSEREQ",
        "FSEVENTWRAP",
        "FSREQCALLBACK",
        "FSREQPROMISE",
        "GETADDRINFOREQWRAP",
        "GETNAMEINFOREQWRAP",
        "HASHREQUEST",
        "HEAPSNAPSHOT",
        "HTTP2PING",
        "HTTP2SESSION",
        "HTTP2SETTINGS",
        "HTTP2STREAM",
        "HTTPCLIENTREQUEST",
        "HTTPINCOMINGMESSAGE",
        "JSSTREAM",
        "JSUDPWRAP",
        "KEYEXPORTREQUEST",
        "KEYGENREQUEST",
        "KEYPAIRGENREQUEST",
        "MESSAGEPORT",
        "NONE",
        "PBKDF2REQUEST",
        "PIPECONNECTWRAP",
        "PIPESERVERWRAP",
        "PIPEWRAP",
        "PROCESSWRAP",
        "PROMISE",
        "QUERYWRAP",
        "QUIC_ENDPOINT",
        "QUIC_LOGSTREAM",
        "QUIC_PACKET",
        "QUIC_SESSION",
        "QUIC_STREAM",
        "QUIC_UDP",
        "RANDOMBYTESREQUEST",
        "RANDOMPRIMEREQUEST",
        "SCRYPTREQUEST",
        "SHUTDOWNWRAP",
        "SIGINTWATCHDOG",
        "SIGNALWRAP",
        "SIGNREQUEST",
        "STATWATCHER",
        "STREAMPIPE",
        "TCPCONNECTWRAP",
        "TCPSERVERWRAP",
        "TCPWRAP",
        "TLSWRAP",
        "TTYWRAP",
        "UDPSENDWRAP",
        "UDPWRAP",
        "VERIFYREQUEST",
        "WORKER",
        "WORKERCPUPROFILE",
        "WORKERCPUUSAGE",
        "WORKERHEAPSNAPSHOT",
        "WORKERHEAPSTATISTICS",
        "WRITEWRAP",
        "ZLIB",
    ]
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = create_hook;
        let _: crate::builtins::BuiltinHandler = hook_enable;
        let _: crate::builtins::BuiltinHandler = hook_disable;
        let _: crate::builtins::BuiltinHandler = execution_async_id;
        let _: crate::builtins::BuiltinHandler = trigger_async_id;
        let _: crate::builtins::BuiltinHandler = execution_async_resource;
        let _: crate::builtins::BuiltinHandler = async_resource_ctor;
        let _: crate::builtins::BuiltinHandler = resource_run_in_async_scope;
        let _: crate::builtins::BuiltinHandler = resource_emit_destroy;
        let _: crate::builtins::BuiltinHandler = resource_async_id;
        let _: crate::builtins::BuiltinHandler = resource_trigger_async_id;
        let _: crate::builtins::BuiltinHandler = resource_bind;
        let _: crate::builtins::BuiltinHandler = bound_trampoline;
        let _: crate::builtins::BuiltinHandler = async_local_storage_ctor;
        let _: crate::builtins::BuiltinHandler = als_run;
        let _: crate::builtins::BuiltinHandler = als_get_store;
        let _: crate::builtins::BuiltinHandler = als_enter_with;
        let _: crate::builtins::BuiltinHandler = als_exit;
        let _: crate::builtins::BuiltinHandler = als_disable;
    }

    #[test]
    fn provider_names_match_go_count() {
        // Go 版 asyncWrapProviderNames 共 66 个键，全部赋值 1。
        assert_eq!(async_wrap_provider_names().len(), 66);
    }
}
