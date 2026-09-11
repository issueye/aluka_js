//! `domain` 内置模块（Phase 8 提前落地）：DEP0003 废弃的 legacy 错误路由。
//!
//! 照实移植 Node.js 22 LTS 规范：
//! - 导出 `create` / `createDomain`（同一函数对象）/ `Domain` / `active`
//!   （初始 null）/ `_stack`（恒为空数组的调试面）；
//! - Domain 实例：`run` / `bind` / `intercept` / `enter` / `exit` / `add` /
//!   `remove` / `members` / `_errorHandler` + EventEmitter 方法面
//!   （on/once/emit/removeListener/listeners/listenerCount/eventNames 等，
//!   自带监听器存储，与 events 模块互不影响）；
//! - `process.domain` 与模块级 `active` 随 enter/exit 更新（初始 null，
//!   全部退出后 undefined——与 Go/Node 一致）；
//! - 错误路由：`intercept` 包装首参为 Error 时路由到 domain 的 `error`
//!   事件；`add(emitter)` 后 emitter 的 `error` 事件经内部转发监听器路由到
//!   domain；`emit('error')` 无监听器时抛原值；
//! - `run` / `bind` 回调抛错时【不】自动 exit（错误向调用方传播，domain
//!   保持 enter 状态——复刻 Node/Go 的共享栈污染行为）；
//! - `exit` 从栈中弹出本 domain 及其上全部条目（`stack[..idx]`，照 Go）。
//!
//! 已知偏离（引擎能力边界）：`bind` / `intercept` 返回的包装函数无法像 Go
//! 那样以闭包捕获各自 domain——静态表保存「最近创建」的同族包装上下文
//! （bind 与 intercept 分槽，互不干扰）；`create()` 实例不带
//! `Domain.prototype` 链（与 Go 一致，`instanceof` 仅对 `new Domain()` 成立）。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// 模块级共享状态（Node 的模块级 stack / active）。
struct DomainGlobal {
    /// 已 enter 的 domain 实例句柄栈
    stack: Vec<u32>,
    /// 当前活动 domain（初始 null；全部退出后 undefined）
    active: Value,
}

impl Default for DomainGlobal {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            active: Value::Null,
        }
    }
}

/// domain 实例内部的 EventEmitter 状态。
#[derive(Default)]
struct DomainState {
    /// 事件名 → 监听器（once 标记在触发时自移除）
    listeners: HashMap<String, Vec<DomainListener>>,
    /// getMaxListeners/setMaxListeners 的上限值（默认 10，仅存储不告警）
    max_listeners: i64,
    /// 已绑定的 emitter 成员（members 数组的宿主）
    members: Vec<Value>,
    /// members JS 数组句柄（add/remove 时同步刷新）
    members_arr: u32,
    /// 已注册内部 error 转发监听器的 emitter → 转发回调
    forwarders: HashMap<u32, Value>,
}

/// 监听器条目。
#[derive(Clone)]
struct DomainListener {
    /// 监听回调
    callback: Value,
    /// once 注册：触发一次后自移除
    once: bool,
}

/// `bind` / `intercept` 包装函数捕获的上下文（静态表模拟 Go 闭包）。
#[derive(Clone)]
struct WrapCtx {
    /// 所属 domain 实例句柄
    domain: u32,
    /// 被包装回调
    cb: Value,
}

/// 模块导出对象句柄（enter/exit 更新 `active` 属性用）。
static MODULE_ID: AtomicU32 = AtomicU32::new(0);
/// Domain.prototype 句柄（`new Domain()` 实例的原型链）。
static PROTO_ID: AtomicU32 = AtomicU32::new(0);
// 模块级全局状态。
thread_local! {
    // GLOBAL：线程局部（堆句柄仅本线程 Vm 有效）。
    static GLOBAL: RefCell<Option<DomainGlobal>> = const { RefCell::new(None) };
}
// domain 实例表：实例句柄 → 状态。
thread_local! {
    // DOMAINS：线程局部（堆句柄仅本线程 Vm 有效）。
    static DOMAINS: RefCell<Option<HashMap<u32, DomainState>>> = const { RefCell::new(None) };
}
// emitter 句柄 → 所属 domain 句柄（内部 error 转发路由表）。
thread_local! {
    // FORWARDER_OF：线程局部（堆句柄仅本线程 Vm 有效）。
    static FORWARDER_OF: RefCell<Option<HashMap<u32, u32>>> = const { RefCell::new(None) };
}
// 最近创建的 bind 包装上下文。
thread_local! {
    // LAST_BIND：线程局部（堆句柄仅本线程 Vm 有效）。
    static LAST_BIND: RefCell<Option<WrapCtx>> = const { RefCell::new(None) };
}
// 最近创建的 intercept 包装上下文。
thread_local! {
    // LAST_INTERCEPT：线程局部（堆句柄仅本线程 Vm 有效）。
    static LAST_INTERCEPT: RefCell<Option<WrapCtx>> = const { RefCell::new(None) };
}
/// DEP0003 弃用警告只发一次。
static DEPRECATION_EMITTED: AtomicBool = AtomicBool::new(false);

/// `require("domain")` / `require("node:domain")`。
pub const MODULE: ModuleDef = ModuleDef {
    name: "domain",
    build,
};

/// 全局状态访问。
fn with_global<R>(f: impl FnOnce(&mut DomainGlobal) -> R) -> R {
    GLOBAL.with(|g| f(g.borrow_mut().get_or_insert_with(DomainGlobal::default)))
}

/// 实例状态访问。
fn with_domain<R>(id: u32, f: impl FnOnce(&mut DomainState) -> R) -> Option<R> {
    DOMAINS.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .get_mut(&id)
            .map(f)
    })
}

/// 首次使用时发出 DEP0003 弃用警告（对齐 Go 的 EmitDeprecation 文本）。
fn ensure_deprecation() {
    if !DEPRECATION_EMITTED.swap(true, Ordering::SeqCst) {
        eprintln!(
            "(aluka) [DEP0003] DeprecationWarning: The domain module is deprecated. \
Use alternative error handling solutions instead."
        );
    }
}

/// 当前接收者（domain 实例）句柄 id。
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

/// 值是否为堆字符串对象。
fn is_heap_string(vm: &Vm, val: Value) -> bool {
    matches!(val.case(), ValueCase::Object(sr)
            if matches!(vm.heap.get(sr.0 as usize), Some(HeapObject::String(_)))
    )
}

/// 值是否可视为 Error（对齐 Go 的 isErrorLike：对象且带字符串 name/message）。
fn is_error_like(vm: &mut Vm, v: Value) -> bool {
    let ValueCase::Object(r) = v.case() else {
        return false;
    };
    if !matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Ordinary { .. })) {
        return false;
    }
    match (vm.get_property(v, "name"), vm.get_property(v, "message")) {
        (Ok(name), Ok(message)) => is_heap_string(vm, name) && is_heap_string(vm, message),
        _ => false,
    }
}

/// 抛出 TypeError（对齐 Go 的 `engine.ErrTypeError: fn must be a function`）。
fn type_error(vm: &mut Vm, msg: &str) -> VmError {
    let err = vm.alloc_error_instance(msg);
    let name = vm.alloc_string("TypeError".to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
    VmError::Thrown(Value::Object(err))
}

/// 更新全局 process.domain。
fn set_process_domain(vm: &mut Vm, v: Value) {
    if let Some(proc_ref) = vm.process_object {
        let _ = vm.set_property(Value::Object(proc_ref), "domain", v);
    }
}

/// 更新模块导出对象的 `active` 属性。
fn set_module_active(vm: &mut Vm, v: Value) {
    let id = MODULE_ID.load(Ordering::SeqCst);
    if id != 0 {
        let _ = set_module_prop(vm, ObjectRef(id), "active", v);
    }
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    MODULE_ID.store(obj.0, Ordering::SeqCst);
    GLOBAL.with(|g| {
        *g.borrow_mut() = Some(DomainGlobal {
            stack: Vec::new(),
            active: Value::Null,
        });
    });

    // create / createDomain：同一函数对象（Node 别名语义）
    let create_ref = vm.alloc_native_fn("domain.create");
    set_module_prop(vm, obj, "create", Value::Object(create_ref))?;
    set_module_prop(vm, obj, "createDomain", Value::Object(create_ref))?;
    register_handler(registry, "domain", "create", domain_create);
    register_handler(registry, "domain", "createDomain", domain_create);

    // Domain 类：new Domain() 实例带 prototype 链（instanceof 支持）
    let proto = vm.alloc_ordinary();
    let ctor = vm.alloc_native_ctor("Domain", Some(proto));
    let _ = vm.set_property(Value::Object(proto), "constructor", Value::Object(ctor));
    set_module_prop(vm, obj, "Domain", Value::Object(ctor))?;
    PROTO_ID.store(proto.0, Ordering::SeqCst);
    // 构造器名即分派键（do_construct 按 NativeCtor 名查表）
    registry.dispatch.insert("Domain".to_owned(), domain_ctor);

    // active：初始 null；_stack：内部调试面（与 Go 一致，恒为空数组）
    set_module_prop(vm, obj, "active", Value::Null)?;
    let stack_arr = vm.alloc_array(Vec::new());
    set_module_prop(vm, obj, "_stack", Value::Object(stack_arr))?;

    // 实例方法面
    for (method, handler) in [
        ("enter", domain_enter as BuiltinHandler),
        ("exit", domain_exit),
        ("run", domain_run),
        ("bind", domain_bind),
        ("intercept", domain_intercept),
        ("add", domain_add),
        ("remove", domain_remove),
        ("_errorHandler", domain_error_handler),
        ("on", emitter_on),
        ("addListener", emitter_on),
        ("once", emitter_once),
        ("off", emitter_off),
        ("removeListener", emitter_off),
        ("removeAllListeners", emitter_remove_all),
        ("listeners", emitter_listeners),
        ("rawListeners", emitter_listeners),
        ("listenerCount", emitter_listener_count),
        ("eventNames", emitter_event_names),
        ("emit", emitter_emit),
        ("setMaxListeners", emitter_set_max_listeners),
        ("getMaxListeners", emitter_get_max_listeners),
        ("prependListener", emitter_prepend),
        ("prependOnceListener", emitter_prepend_once),
    ] {
        register_handler(registry, "domain:instance", method, handler);
    }
    // bind / intercept 返回的包装函数（裸调用，无接收者）
    registry
        .dispatch
        .insert("domain:runBound".to_owned(), run_bound_trampoline);
    registry.dispatch.insert(
        "domain:runIntercepted".to_owned(),
        run_intercepted_trampoline,
    );
    // add() 注册到 emitter 上的内部 error 转发监听器（接收者=emitter）
    registry
        .dispatch
        .insert("domain:errorForwarder".to_owned(), error_forwarder);

    Ok(obj)
}

/// 创建 domain 实例对象（`with_proto`：`new Domain()` 挂 prototype 链）。
fn new_instance(vm: &mut Vm, with_proto: bool) -> Value {
    let proto = {
        let id = PROTO_ID.load(Ordering::SeqCst);
        if with_proto && id != 0 {
            Some(ObjectRef(id))
        } else {
            None
        }
    };
    let obj = if with_proto {
        vm.alloc_ordinary_with_exact_proto(proto)
    } else {
        vm.alloc_ordinary()
    };
    let ns = Value::Object(vm.alloc_string("domain:instance".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", ns);
    // members：构造时初始化的自有数组属性
    let members_arr = vm.alloc_array(Vec::new());
    let _ = vm.set_property(Value::Object(obj), "members", Value::Object(members_arr));
    // domain 属性：Node/Go 中初始化为 null
    let _ = vm.set_property(Value::Object(obj), "domain", Value::Null);
    for method in [
        "enter",
        "exit",
        "run",
        "bind",
        "intercept",
        "add",
        "remove",
        "_errorHandler",
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "removeAllListeners",
        "listeners",
        "rawListeners",
        "listenerCount",
        "eventNames",
        "emit",
        "setMaxListeners",
        "getMaxListeners",
        "prependListener",
        "prependOnceListener",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("domain:instance.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    DOMAINS.with(|g| {
        g.borrow_mut().get_or_insert_with(HashMap::new).insert(
            obj.0,
            DomainState {
                listeners: HashMap::new(),
                max_listeners: 10,
                members: Vec::new(),
                members_arr: members_arr.0,
                forwarders: HashMap::new(),
            },
        );
    });
    Value::Object(obj)
}

/// `domain.create()` / `domain.createDomain()`。
fn domain_create(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    ensure_deprecation();
    Ok(new_instance(vm, false))
}

/// `new Domain()`（构造器）。
fn domain_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    ensure_deprecation();
    Ok(new_instance(vm, true))
}

/// `enter()`：压栈并设为活动 domain（同步 process.domain 与模块 active）。
fn domain_enter(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let self_val = Value::Object(ObjectRef(id));
    with_global(|st| {
        st.active = self_val;
        st.stack.push(id);
    });
    set_process_domain(vm, self_val);
    set_module_active(vm, self_val);
    Ok(Value::Undefined)
}

/// `exit()`：从栈中弹出本 domain 及其上全部条目（`stack[..idx]`，照 Go）。
fn domain_exit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let idx = with_global(|st| st.stack.iter().rposition(|&x| x == id));
    let Some(idx) = idx else {
        return Ok(Value::Undefined);
    };
    let new_active = with_global(|st| {
        st.stack.truncate(idx);
        st.active = st
            .stack
            .last()
            .map(|&x| Value::Object(ObjectRef(x)))
            .unwrap_or(Value::Undefined);
        st.active
    });
    set_process_domain(vm, new_active);
    set_module_active(vm, new_active);
    Ok(Value::Undefined)
}

/// `run(fn, ...args)`：enter 后调用 fn；成功 exit 并返回结果；失败【不】exit。
fn domain_run(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let fn_val = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, fn_val) {
        return Err(type_error(vm, "fn must be a function"));
    }
    let id = receiver_id().unwrap_or(0);
    let self_val = Value::Object(ObjectRef(id));
    // enter（与 enter 方法同一套栈逻辑）
    with_global(|st| {
        st.active = self_val;
        st.stack.push(id);
    });
    set_process_domain(vm, self_val);
    set_module_active(vm, self_val);
    let rest: Vec<Value> = args.iter().skip(1).copied().collect();
    match vm.invoke_callable(fn_val, Value::Undefined, &rest) {
        Ok(ret) => {
            exit_instance(vm, id);
            Ok(ret)
        }
        // 复刻 Node/Go 语义：错误传播，domain 保持 enter 状态（栈污染）
        Err(err) => Err(err),
    }
}

/// 从全局栈中弹出指定 domain 及其上全部条目并同步活动状态。
fn exit_instance(vm: &mut Vm, id: u32) {
    let Some(idx) = with_global(|st| st.stack.iter().rposition(|&x| x == id)) else {
        return;
    };
    let new_active = with_global(|st| {
        st.stack.truncate(idx);
        st.active = st
            .stack
            .last()
            .map(|&x| Value::Object(ObjectRef(x)))
            .unwrap_or(Value::Undefined);
        st.active
    });
    set_process_domain(vm, new_active);
    set_module_active(vm, new_active);
}

/// `bind(fn)`：返回包装函数；调用时 enter/调用/exit，抛错不 exit。
fn domain_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(type_error(vm, "fn must be a function"));
    }
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    LAST_BIND.with(|g| *g.borrow_mut() = Some(WrapCtx { domain: id, cb }));
    Ok(Value::Object(vm.alloc_native_fn("domain:runBound")))
}

/// bind 包装调用：enter → 调用 → exit；抛错原样传播（domain 保持 enter）。
fn run_bound_trampoline(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(ctx) = LAST_BIND.with(|g| g.borrow().clone()) else {
        return Ok(Value::Undefined);
    };
    let self_val = Value::Object(ObjectRef(ctx.domain));
    with_global(|st| {
        st.active = self_val;
        st.stack.push(ctx.domain);
    });
    set_process_domain(vm, self_val);
    set_module_active(vm, self_val);
    match vm.invoke_callable(ctx.cb, Value::Undefined, args) {
        Ok(ret) => {
            exit_instance(vm, ctx.domain);
            Ok(ret)
        }
        Err(err) => Err(err),
    }
}

/// `intercept(cb)`：包装首参为 Error 时路由到 domain `error` 事件（cb 不
/// 调用）；否则 enter / 以去掉首参的实参调用 / exit。
fn domain_intercept(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(type_error(vm, "fn must be a function"));
    }
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    LAST_INTERCEPT.with(|g| *g.borrow_mut() = Some(WrapCtx { domain: id, cb }));
    Ok(Value::Object(vm.alloc_native_fn("domain:runIntercepted")))
}

/// intercept 包装调用。
fn run_intercepted_trampoline(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(ctx) = LAST_INTERCEPT.with(|g| g.borrow().clone()) else {
        return Ok(Value::Undefined);
    };
    let first = args.first().copied().unwrap_or(Value::Undefined);
    if is_error_like(vm, first) {
        if let Some(er) = first.as_object() {
            let self_val = Value::Object(ObjectRef(ctx.domain));
            let _ = vm.set_property(Value::Object(er), "domainBound", ctx.cb);
            let _ = vm.set_property(Value::Object(er), "domainThrown", Value::Boolean(false));
            let _ = vm.set_property(Value::Object(er), "domain", self_val);
        }
        return emit_error(vm, ctx.domain, first);
    }
    let self_val = Value::Object(ObjectRef(ctx.domain));
    with_global(|st| {
        st.active = self_val;
        st.stack.push(ctx.domain);
    });
    set_process_domain(vm, self_val);
    set_module_active(vm, self_val);
    let rest: Vec<Value> = args.iter().skip(1).copied().collect();
    match vm.invoke_callable(ctx.cb, Value::Undefined, &rest) {
        Ok(ret) => {
            exit_instance(vm, ctx.domain);
            Ok(ret)
        }
        Err(err) => Err(err),
    }
}

/// `add(emitter)`：绑定 emitter（转移旧绑定），登记内部 `error` 转发监听器。
fn domain_add(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let Some(ee) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let self_val = Value::Object(ObjectRef(id));
    // 已绑定本 domain：直接返回
    if let Some(er) = ee.as_object() {
        if vm.get_property(ee, "domain").ok() == Some(self_val) {
            return Ok(Value::Undefined);
        }
        // 已有旧 domain：先经旧 domain 的 remove 解除
        if let Ok(old) = vm.get_property(ee, "domain") {
            if let Some(old_ref) = old.as_object() {
                if old_ref.0 != er.0 {
                    if let Ok(remove_fn) = vm.get_property(old, "remove") {
                        if is_function(vm, remove_fn) {
                            let _ = vm.invoke_callable(remove_fn, old, &[ee]);
                        }
                    }
                }
            }
        }
    }
    if let Some(er) = ee.as_object() {
        let _ = vm.set_property(ee, "domain", self_val);
        // 注册内部 'error' 转发监听器（ee.emit('error') → 本 domain 路由）
        if let Ok(on_fn) = vm.get_property(ee, "on") {
            if is_function(vm, on_fn) {
                let forwarder = Value::Object(vm.alloc_native_fn("domain:errorForwarder"));
                let event = Value::Object(vm.alloc_string("error".to_owned()));
                let _ = vm.invoke_callable(on_fn, ee, &[event, forwarder]);
                with_domain(id, |st| {
                    st.forwarders.insert(er.0, forwarder);
                });
                FORWARDER_OF.with(|g| {
                    g.borrow_mut()
                        .get_or_insert_with(HashMap::new)
                        .insert(er.0, id);
                });
            }
        }
    }
    with_domain(id, |st| st.members.push(ee));
    refresh_members(vm, id);
    Ok(Value::Undefined)
}

/// `remove(emitter)`：解除绑定并移除转发监听器。
fn domain_remove(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let Some(ee) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    if let Some(er) = ee.as_object() {
        let _ = vm.set_property(ee, "domain", Value::Null);
        let forwarder = with_domain(id, |st| st.forwarders.remove(&er.0)).flatten();
        FORWARDER_OF.with(|g| {
            g.borrow_mut()
                .get_or_insert_with(HashMap::new)
                .remove(&er.0);
        });
        if let Some(fwd) = forwarder {
            if let Ok(off_fn) = vm.get_property(ee, "removeListener") {
                if is_function(vm, off_fn) {
                    let event = Value::Object(vm.alloc_string("error".to_owned()));
                    let _ = vm.invoke_callable(off_fn, ee, &[event, fwd]);
                }
            }
        }
    }
    with_domain(id, |st| {
        if let Some(pos) = st.members.iter().position(|m| *m == ee) {
            st.members.remove(pos);
        }
    });
    refresh_members(vm, id);
    Ok(Value::Undefined)
}

/// 同步 members JS 数组内容（add/remove 后刷新）。
fn refresh_members(vm: &mut Vm, id: u32) {
    let Some(arr_id) = with_domain(id, |st| st.members_arr) else {
        return;
    };
    let members = with_domain(id, |st| st.members.clone()).unwrap_or_default();
    if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(arr_id as usize) {
        *elements = members;
    }
}

/// 内部 error 转发监听器：ee.emit('error', er) → 所属 domain 错误路由。
fn error_forwarder(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(emitter_id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let Some(domain_id) = FORWARDER_OF.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .get(&emitter_id)
            .copied()
    }) else {
        return Ok(Value::Undefined);
    };
    let er = args.first().copied().unwrap_or(Value::Undefined);
    let self_val = Value::Object(ObjectRef(domain_id));
    if let Some(eo) = er.as_object() {
        let _ = vm.set_property(
            Value::Object(eo),
            "domainEmitter",
            Value::Object(ObjectRef(emitter_id)),
        );
        let _ = vm.set_property(Value::Object(eo), "domain", self_val);
        let _ = vm.set_property(Value::Object(eo), "domainThrown", Value::Boolean(false));
    }
    emit_error(vm, domain_id, er)
}

/// `_errorHandler([er])`：设置错误属性、弹出本 domain、路由到 error 事件。
fn domain_error_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Boolean(false));
    };
    let Some(er) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    if let Some(eo) = er.as_object() {
        let self_val = Value::Object(ObjectRef(id));
        let _ = vm.set_property(Value::Object(eo), "domain", self_val);
        let _ = vm.set_property(Value::Object(eo), "domainThrown", Value::Boolean(true));
    }
    // 弹出当前活动 domain（及其相邻重复）
    loop {
        let top = with_global(|st| st.stack.last().copied());
        if top != Some(id) {
            break;
        }
        exit_instance(vm, id);
    }
    emit_error(vm, id, er)
}

/// 触发 domain 的 `error` 事件；无监听器时抛原错误值（Node 语义）。
fn emit_error(vm: &mut Vm, id: u32, er: Value) -> Result<Value, VmError> {
    let listeners = with_domain(id, |st| {
        st.listeners
            .get("error")
            .map(|ls| ls.iter().map(|l| l.callback).collect::<Vec<_>>())
            .unwrap_or_default()
    })
    .unwrap_or_default();
    if listeners.is_empty() {
        return Err(VmError::Thrown(er));
    }
    for cb in listeners {
        vm.invoke_callable(cb, Value::Undefined, &[er])?;
    }
    Ok(Value::Boolean(true))
}

// --- EventEmitter 方法面（自带监听器存储，语义对齐 Go 的 domainEmitter） ---

/// `on(event, listener)` / `addListener`。
fn emitter_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 && is_function(vm, args[1]) {
        let event = vm.format_value(args[0]);
        let listener = args[1];
        with_domain(id, |st| {
            st.listeners.entry(event).or_default().push(DomainListener {
                callback: listener,
                once: false,
            });
        });
    }
    Ok(current_receiver())
}

/// `once(event, listener)`。
fn emitter_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 && is_function(vm, args[1]) {
        let event = vm.format_value(args[0]);
        let listener = args[1];
        with_domain(id, |st| {
            st.listeners.entry(event).or_default().push(DomainListener {
                callback: listener,
                once: true,
            });
        });
    }
    Ok(current_receiver())
}

/// `prependListener(event, listener)`。
fn emitter_prepend(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    prepend_impl(vm, args, false)
}

/// `prependOnceListener(event, listener)`。
fn emitter_prepend_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    prepend_impl(vm, args, true)
}

/// 头部插入监听器公共实现。
fn prepend_impl(vm: &mut Vm, args: &[Value], once: bool) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 && is_function(vm, args[1]) {
        let event = vm.format_value(args[0]);
        let listener = args[1];
        with_domain(id, |st| {
            let list = st.listeners.entry(event).or_default();
            list.insert(
                0,
                DomainListener {
                    callback: listener,
                    once,
                },
            );
        });
    }
    Ok(current_receiver())
}

/// `off(event, listener)` / `removeListener`。
fn emitter_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let target = args[1];
        with_domain(id, |st| {
            if let Some(list) = st.listeners.get_mut(&event) {
                if let Some(pos) = list.iter().position(|l| l.callback == target) {
                    list.remove(pos);
                }
            }
        });
    }
    Ok(current_receiver())
}

/// `removeAllListeners([event])`。
fn emitter_remove_all(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    match args.first() {
        Some(event) => {
            let name = vm.format_value(*event);
            with_domain(id, |st| {
                st.listeners.remove(&name);
            });
        }
        None => {
            with_domain(id, |st| {
                st.listeners.clear();
            });
        }
    }
    Ok(current_receiver())
}

/// `listeners(event)` / `rawListeners(event)`：回调副本数组。
fn emitter_listeners(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    let event = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let callbacks = with_domain(id, |st| {
        st.listeners
            .get(&event)
            .map(|ls| ls.iter().map(|l| l.callback).collect::<Vec<_>>())
            .unwrap_or_default()
    })
    .unwrap_or_default();
    Ok(Value::Object(vm.alloc_array(callbacks)))
}

/// `listenerCount([event])`：带事件名为该事件计数，缺省为总数。
fn emitter_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Number(0.0));
    };
    let count = match args.first() {
        Some(event) => {
            let name = vm.format_value(*event);
            with_domain(id, |st| st.listeners.get(&name).map(Vec::len).unwrap_or(0)).unwrap_or(0)
        }
        None => with_domain(id, |st| st.listeners.values().map(Vec::len).sum()).unwrap_or(0),
    };
    Ok(Value::Number(count as f64))
}

/// `eventNames()`：非空监听器的事件名数组（字典序输出，保证确定性；
/// Go 的 map 遍历序随机，无可对拍顺序）。
fn emitter_event_names(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    let mut names = with_domain(id, |st| {
        st.listeners
            .iter()
            .filter(|(_, ls)| !ls.is_empty())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    names.sort();
    let elems: Vec<Value> = names
        .into_iter()
        .map(|n| Value::Object(vm.alloc_string(n)))
        .collect();
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `emit(event, ...args)`：error 事件无监听器时抛原值；其余返回是否有监听。
fn emitter_emit(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Boolean(false));
    };
    let Some(event_val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let event = vm.format_value(*event_val);
    if event == "error" {
        let count = with_domain(id, |st| {
            st.listeners.get("error").map(Vec::len).unwrap_or(0)
        })
        .unwrap_or(0);
        if count == 0 {
            let er = args.get(1).copied().unwrap_or(Value::Undefined);
            return Err(VmError::Thrown(er));
        }
        let listeners = with_domain(id, |st| {
            st.listeners
                .get("error")
                .map(|ls| ls.iter().map(|l| l.callback).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .unwrap_or_default();
        let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
        for cb in listeners {
            vm.invoke_callable(cb, Value::Undefined, &emit_args)?;
        }
        return Ok(Value::Boolean(true));
    }
    // 快照触发；once 条目触发后自移除
    let (listeners, once_flags) = with_domain(id, |st| {
        let Some(list) = st.listeners.get_mut(&event) else {
            return (Vec::new(), Vec::new());
        };
        let mut cbs = Vec::with_capacity(list.len());
        let mut once_flags = Vec::with_capacity(list.len());
        for l in list.iter() {
            cbs.push(l.callback);
            once_flags.push(l.once);
        }
        list.retain(|l| !l.once);
        (cbs, once_flags)
    })
    .unwrap_or_default();
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
    for cb in listeners {
        vm.invoke_callable(cb, Value::Undefined, &emit_args)?;
    }
    Ok(Value::Boolean(!once_flags.is_empty()))
}

/// `setMaxListeners(n)`。
fn emitter_set_max_listeners(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if let Some(n) = args.first().and_then(|v| v.as_number()) {
        with_domain(id, |st| st.max_listeners = n as i64);
    }
    Ok(current_receiver())
}

/// `getMaxListeners()`。
fn emitter_get_max_listeners(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Number(10.0));
    };
    let max = with_domain(id, |st| st.max_listeners).unwrap_or(10);
    Ok(Value::Number(max as f64))
}

/// 编译期锚定：确保处理器签名与注册表一致。
/// GC 根快照：domain 活动实例、成员与转发回调。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    GLOBAL.with(|g| {
        if let Some(g) = g.borrow().as_ref() {
            out.push(g.active);
        }
    });
    DOMAINS.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for d in map.values() {
                for v in &d.members {
                    out.push(*v);
                }
                for v in d.forwarders.values() {
                    out.push(*v);
                }
                for items in d.listeners.values() {
                    for l in items {
                        out.push(l.callback);
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = domain_create;
        let _: crate::builtins::BuiltinHandler = domain_ctor;
        let _: crate::builtins::BuiltinHandler = domain_enter;
        let _: crate::builtins::BuiltinHandler = domain_exit;
        let _: crate::builtins::BuiltinHandler = domain_run;
        let _: crate::builtins::BuiltinHandler = domain_bind;
        let _: crate::builtins::BuiltinHandler = domain_intercept;
        let _: crate::builtins::BuiltinHandler = domain_add;
        let _: crate::builtins::BuiltinHandler = domain_remove;
        let _: crate::builtins::BuiltinHandler = domain_error_handler;
        let _: crate::builtins::BuiltinHandler = run_bound_trampoline;
        let _: crate::builtins::BuiltinHandler = run_intercepted_trampoline;
        let _: crate::builtins::BuiltinHandler = error_forwarder;
        let _: crate::builtins::BuiltinHandler = emitter_on;
        let _: crate::builtins::BuiltinHandler = emitter_once;
        let _: crate::builtins::BuiltinHandler = emitter_off;
        let _: crate::builtins::BuiltinHandler = emitter_remove_all;
        let _: crate::builtins::BuiltinHandler = emitter_listeners;
        let _: crate::builtins::BuiltinHandler = emitter_listener_count;
        let _: crate::builtins::BuiltinHandler = emitter_event_names;
        let _: crate::builtins::BuiltinHandler = emitter_emit;
        let _: crate::builtins::BuiltinHandler = emitter_set_max_listeners;
        let _: crate::builtins::BuiltinHandler = emitter_get_max_listeners;
        let _: crate::builtins::BuiltinHandler = emitter_prepend;
        let _: crate::builtins::BuiltinHandler = emitter_prepend_once;
    }
}
