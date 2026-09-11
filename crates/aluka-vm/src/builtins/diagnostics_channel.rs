//! `diagnostics_channel` 内置模块（Phase 7）：诊断通道发布/订阅。
//!
//! 语义逐字对齐 Node.js 22 LTS 规范：
//! - `channel(name)`：命名通道注册表，同名重复调用返回同一实例对象；
//! - 实例方法 `subscribe` / `unsubscribe` / `publish` / `bindStore` /
//!   `unbindStore` / `runStores`；实例另有自有布尔属性 `hasSubscribers`
//!   （订阅后翻转，遮蔽原型同名方法——与 Go 一致，不可作为方法调用）；
//! - 模块级 `channel` / `hasSubscribers` / `subscribe` / `unsubscribe` 与
//!   `Channel` 构造器（调用即抛错：Node 不暴露 Channel 构造器）；
//! - `tracingChannel(name)`：`tracing:<name>:{start,end,asyncStart,asyncEnd,error}`
//!   五命名通道的聚合面 + `traceSync` / `tracePromise` / `traceCallback`
//!   （Go 中三者共用同一同步实现，回调实参取 `args[3:]`——照实移植）；
//! - `bindStore` 绑定 AsyncLocalStorage：`runStores` 逐层经 store.run 进入
//!   绑定上下文；`publish` 时订阅者在调用方上下文执行（Node 22 语义）。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// 单个命名通道的注册表状态（对象、订阅者、tracing 观察者、绑定 store）。
struct ChannelState {
    /// 通道名（`tracing:<name>:<phase>` 也占名字空间）
    name: String,
    /// 实例对象句柄（`channel(name)` 复用同一对象）
    obj: ObjectRef,
    /// 订阅回调列表（按订阅顺序触发）
    subscribers: Vec<Value>,
    /// 引用本通道的 tracingChannel 聚合对象（`hasSubscribers` 联动更新）
    watchers: Vec<u32>,
    /// `bindStore` 绑定的 AsyncLocalStorage 实例（重绑移到末尾）
    stores: Vec<Value>,
}

/// tracingChannel 聚合对象状态：成员通道句柄列表。
struct TracingState {
    /// start/end/asyncStart/asyncEnd/error 五个成员通道的实例句柄
    members: Vec<u32>,
}

// 命名通道注册表：实例对象句柄 → 状态。
thread_local! {
    // CHANNELS：线程局部（堆句柄仅本线程 Vm 有效）。
    static CHANNELS: RefCell<Option<HashMap<u32, ChannelState>>> = const { RefCell::new(None) };
}
// 通道名 → 实例对象句柄（`channel(name)` 幂等）。
thread_local! {
    // CHANNEL_NAMES：线程局部（堆句柄仅本线程 Vm 有效）。
    static CHANNEL_NAMES: RefCell<Option<HashMap<String, u32>>> = const { RefCell::new(None) };
}
// tracingChannel 聚合对象句柄 → 成员通道状态。
thread_local! {
    // TRACINGS：线程局部（堆句柄仅本线程 Vm 有效）。
    static TRACINGS: RefCell<Option<HashMap<u32, TracingState>>> = const { RefCell::new(None) };
}
// `runStores` 链式续体（wrapped 回调）捕获的剩余状态，LIFO 消费。
thread_local! {
    // CHAIN：线程局部（堆句柄仅本线程 Vm 有效）。
    static CHAIN: RefCell<Vec<ChainCtx>> = const { RefCell::new(Vec::new()) };
}

/// `runStores` 链式调用中被 store.run 持有的续体上下文。
struct ChainCtx {
    /// 尚未进入的剩余绑定 store（外层先进入）
    remaining: Vec<Value>,
    /// runStores 的上下文值（作为每个 store 的运行值）
    context: Value,
    /// 最终回调
    callback: Value,
}

/// `require("diagnostics_channel")` / `require("node:diagnostics_channel")`。
pub const MODULE: ModuleDef = ModuleDef {
    name: "diagnostics_channel",
    build,
};

/// 可加锁访问的静态表便捷宏展开辅助：`with_map!` 风格手动展开。
fn with_channels<R>(f: impl FnOnce(&mut HashMap<u32, ChannelState>) -> R) -> R {
    CHANNELS.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// 通道名注册表访问。
fn with_names<R>(f: impl FnOnce(&mut HashMap<String, u32>) -> R) -> R {
    CHANNEL_NAMES.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// tracing 聚合表访问。
fn with_tracings<R>(f: impl FnOnce(&mut HashMap<u32, TracingState>) -> R) -> R {
    TRACINGS.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// 值是否为可调用函数（对齐 Go 的 `Value.IsFunction`）。
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

/// 抛出 JS Error 实例（对齐 Go：native 返回的 error 以 Error 对象呈现，
/// `e.message` 可读）。
fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for (key, name) in [
        ("channel", "diagnostics_channel.channel"),
        ("hasSubscribers", "diagnostics_channel.hasSubscribers"),
        ("subscribe", "diagnostics_channel.subscribe"),
        ("unsubscribe", "diagnostics_channel.unsubscribe"),
        ("tracingChannel", "diagnostics_channel.tracingChannel"),
        ("Channel", "diagnostics_channel.Channel"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, key, Value::Object(fn_ref))?;
    }
    register_handler(registry, "diagnostics_channel", "channel", module_channel);
    register_handler(
        registry,
        "diagnostics_channel",
        "hasSubscribers",
        module_has_subscribers,
    );
    register_handler(
        registry,
        "diagnostics_channel",
        "subscribe",
        module_subscribe,
    );
    register_handler(
        registry,
        "diagnostics_channel",
        "unsubscribe",
        module_unsubscribe,
    );
    register_handler(
        registry,
        "diagnostics_channel",
        "tracingChannel",
        module_tracing_channel,
    );
    register_handler(registry, "diagnostics_channel", "Channel", channel_ctor);

    // 实例方法分派（命名空间 + 具名原生函数，剥离调用亦可分派）
    for (method, handler) in [
        ("subscribe", instance_subscribe as BuiltinHandler),
        ("unsubscribe", instance_unsubscribe),
        ("publish", instance_publish),
        ("bindStore", instance_bind_store),
        ("unbindStore", instance_unbind_store),
        ("runStores", instance_run_stores),
    ] {
        register_handler(registry, "diagnostics_channel:channel", method, handler);
    }
    for method in [
        "subscribe",
        "unsubscribe",
        "traceSync",
        "tracePromise",
        "traceCallback",
    ] {
        let handler = match method {
            "subscribe" => tracing_subscribe,
            "unsubscribe" => tracing_unsubscribe,
            _ => tracing_trace,
        };
        register_handler(registry, "diagnostics_channel:tracing", method, handler);
    }
    // runStores 链式续体：原生函数自名即分派键（非「模块.方法」两级形态），
    // 直接以完整键注册。
    registry.dispatch.insert(
        "diagnostics_channel:runStores.chain".to_owned(),
        run_stores_chain,
    );

    Ok(obj)
}

/// 按名取（或创建）通道实例；`channel(name)` 对同名幂等（同一对象）。
fn get_channel(vm: &mut Vm, name: &str) -> ObjectRef {
    if let Some(id) = with_names(|names| names.get(name).copied()) {
        if with_channels(|channels| channels.contains_key(&id)) {
            return ObjectRef(id);
        }
    }
    let obj = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("diagnostics_channel:channel".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", ns);
    let name_val = Value::Object(vm.alloc_string(name.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "name", name_val);
    let _ = vm.set_property(Value::Object(obj), "hasSubscribers", Value::Boolean(false));
    for method in [
        "subscribe",
        "unsubscribe",
        "publish",
        "bindStore",
        "unbindStore",
        "runStores",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("diagnostics_channel:channel.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    let id = obj.0;
    with_names(|names| names.insert(name.to_owned(), id));
    with_channels(|channels| {
        channels.insert(
            id,
            ChannelState {
                name: name.to_owned(),
                obj,
                subscribers: Vec::new(),
                watchers: Vec::new(),
                stores: Vec::new(),
            },
        );
    });
    obj
}

/// 通道状态只读/可变访问（接收者句柄定位）。
fn channel_of<R>(id: u32, f: impl FnOnce(&mut ChannelState) -> R) -> Option<R> {
    with_channels(|channels| channels.get_mut(&id).map(f))
}

/// `channel([name])`：取（或创建）命名通道实例。
fn module_channel(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    Ok(Value::Object(get_channel(vm, &name)))
}

/// `hasSubscribers(name)`：命名通道是否有订阅者（不存在则创建空通道）。
fn module_has_subscribers(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let id = get_channel(vm, &name).0;
    let has = channel_of(id, |st| !st.subscribers.is_empty()).unwrap_or(false);
    Ok(Value::Boolean(has))
}

/// `subscribe(name, fn)`：模块级订阅。
fn module_subscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.get(1).copied().unwrap_or(Value::Undefined);
    if args.len() < 2 || !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "diagnostics_channel: subscriber must be a function",
        ));
    }
    let name = vm.format_value(args[0]);
    let id = get_channel(vm, &name).0;
    channel_subscribe(vm, id, cb)?;
    Ok(Value::Undefined)
}

/// `unsubscribe(name, fn)`：模块级退订。
fn module_unsubscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Ok(Value::Boolean(false));
    }
    let name = vm.format_value(args[0]);
    let id = get_channel(vm, &name).0;
    let removed = channel_unsubscribe(vm, id, args[1])?;
    Ok(Value::Boolean(removed))
}

/// `Channel()`：Node 不暴露构造器，调用即抛错。
fn channel_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Err(thrown(
        vm,
        "diagnostics_channel: Channel constructor is not exposed; use channel(name)",
    ))
}

/// 订阅：追加回调、翻转自有 `hasSubscribers` 并联动 tracing 观察者。
fn channel_subscribe(vm: &mut Vm, id: u32, cb: Value) -> Result<(), VmError> {
    let obj = channel_of(id, |st| {
        st.subscribers.push(cb);
        st.obj
    });
    if let Some(obj) = obj {
        set_module_prop(vm, obj, "hasSubscribers", Value::Boolean(true))?;
    }
    notify_watchers(vm, id)
}

/// 退订：按身份移除首个匹配；更新自有 `hasSubscribers` 并联动观察者。
fn channel_unsubscribe(vm: &mut Vm, id: u32, cb: Value) -> Result<bool, VmError> {
    let found = channel_of(id, |st| {
        if let Some(pos) = st.subscribers.iter().position(|s| *s == cb) {
            st.subscribers.remove(pos);
            true
        } else {
            false
        }
    })
    .unwrap_or(false);
    if found {
        let remaining = channel_of(id, |st| (st.obj, st.subscribers.len()));
        if let Some((obj, len)) = remaining {
            set_module_prop(vm, obj, "hasSubscribers", Value::Boolean(len > 0))?;
        }
        notify_watchers(vm, id)?;
    }
    Ok(found)
}

/// 联动更新引用本通道的 tracingChannel 聚合对象 `hasSubscribers`。
fn notify_watchers(vm: &mut Vm, channel_id: u32) -> Result<(), VmError> {
    let watchers = channel_of(channel_id, |st| st.watchers.clone()).unwrap_or_default();
    for tracing_id in watchers {
        update_tracing_has_subscribers(vm, tracing_id)?;
    }
    Ok(())
}

/// 重算 tracing 聚合对象的 `hasSubscribers`（任一成员通道有订阅者即为真）。
fn update_tracing_has_subscribers(vm: &mut Vm, tracing_id: u32) -> Result<(), VmError> {
    let members =
        with_tracings(|map| map.get(&tracing_id).map(|t| t.members.clone())).unwrap_or_default();
    let mut has = false;
    for member in members {
        if channel_of(member, |st| !st.subscribers.is_empty()).unwrap_or(false) {
            has = true;
            break;
        }
    }
    set_module_prop(
        vm,
        ObjectRef(tracing_id),
        "hasSubscribers",
        Value::Boolean(has),
    )?;
    Ok(())
}

/// 发布：快照订阅者后逐个以 `(message, name)` 调用；首个错误向调用方传播。
fn channel_publish(vm: &mut Vm, id: u32, message: Value) -> Result<(), VmError> {
    let snapshot = channel_of(id, |st| (st.subscribers.clone(), st.name.clone()));
    let Some((subscribers, name)) = snapshot else {
        return Ok(());
    };
    let name_val = Value::Object(vm.alloc_string(name));
    for subscriber in subscribers {
        if is_function(vm, subscriber) {
            vm.invoke_callable(subscriber, Value::Undefined, &[message, name_val])?;
        }
    }
    Ok(())
}

/// `tracingChannel(name)`：五命名通道聚合 + trace 方法面。
fn module_tracing_channel(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let tracing = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("diagnostics_channel:tracing".to_owned()));
    let _ = vm.set_property(Value::Object(tracing), "_builtinNs", ns);
    let mut members = Vec::new();
    for key in ["start", "end", "asyncStart", "asyncEnd", "error"] {
        let channel = get_channel(vm, &format!("tracing:{name}:{key}"));
        let _ = vm.set_property(Value::Object(tracing), key, Value::Object(channel));
        let channel_id = channel.0;
        channel_of(channel_id, |st| st.watchers.push(tracing.0));
        members.push(channel_id);
    }
    with_tracings(|map| map.insert(tracing.0, TracingState { members }));
    update_tracing_has_subscribers(vm, tracing.0)?;

    for method in [
        "subscribe",
        "unsubscribe",
        "traceSync",
        "tracePromise",
        "traceCallback",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("diagnostics_channel:tracing.{method}"));
        let _ = vm.set_property(Value::Object(tracing), method, Value::Object(fn_ref));
    }
    Ok(Value::Object(tracing))
}

/// tracing 聚合对象的 `subscribe(fn)`：向全部五个成员通道订阅。
fn tracing_subscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(cb) = args.first().copied() {
        let members = tracing_members(current_tracing_id());
        for member in members {
            channel_subscribe(vm, member, cb)?;
        }
    }
    Ok(Value::Undefined)
}

/// tracing 聚合对象的 `unsubscribe(fn)`：从全部成员通道退订。
fn tracing_unsubscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(cb) = args.first().copied() {
        let members = tracing_members(current_tracing_id());
        for member in members {
            channel_unsubscribe(vm, member, cb)?;
        }
    }
    Ok(Value::Undefined)
}

/// `traceSync/tracePromise/traceCallback` 共用入口（Go 同一实现）。
fn tracing_trace(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = current_tracing_id() else {
        return Ok(Value::Undefined);
    };
    trace_call(vm, args, id)
}

/// 当前 tracing 聚合对象的成员通道列表。
fn tracing_members(id: Option<u32>) -> Vec<u32> {
    id.and_then(|id| with_tracings(|map| map.get(&id).map(|t| t.members.clone())))
        .unwrap_or_default()
}

/// `traceSync/tracePromise/traceCallback` 的同步共用实现（Go 同构）：
/// 发布 start → 调用（实参取 `args[3:]`，照实移植）→ 成功发布 end /
/// 失败发布 error（字符串化）并向调用方传播。
fn trace_call(vm: &mut Vm, args: &[Value], tracing_id: u32) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "diagnostics_channel: trace callback must be a function",
        ));
    }
    let mut context = Value::Object(vm.alloc_ordinary());
    if let Some(second) = args.get(1) {
        if !(second.is_undefined() || second.is_null()) {
            context = *second;
        }
    }
    let start = member_channel(tracing_id, 0);
    let end = member_channel(tracing_id, 1);
    let error = member_channel(tracing_id, 4);

    let start_id = start.map(|o| o.0);
    if let Some(sid) = start_id {
        channel_publish(vm, sid, context)?;
    }
    let invoke_args: Vec<Value> = if args.len() > 3 {
        args[3..].to_vec()
    } else {
        Vec::new()
    };
    match vm.invoke_callable(cb, Value::Undefined, &invoke_args) {
        Ok(result) => {
            if let Some(eid) = end.map(|o| o.0) {
                channel_publish(vm, eid, context)?;
            }
            Ok(result)
        }
        Err(err) => {
            if let Some(err_id) = error.map(|o| o.0) {
                let text = describe_thrown(vm, &err);
                let text_val = Value::Object(vm.alloc_string(text));
                channel_publish(vm, err_id, text_val)?;
            }
            Err(err)
        }
    }
}

/// tracing 成员通道句柄（成员按 start/end/asyncStart/asyncEnd/error 顺序）。
fn member_channel(tracing_id: u32, index: usize) -> Option<ObjectRef> {
    with_tracings(|map| {
        map.get(&tracing_id)
            .and_then(|t| t.members.get(index))
            .copied()
            .map(ObjectRef)
    })
}

/// 抛出值的字符串化（对齐 Go `err.Error()` 的首行 `Name: message` 形态）。
fn describe_thrown(vm: &mut Vm, err: &VmError) -> String {
    if let VmError::Thrown(v) = err {
        if let Some(r) = v.as_object() {
            if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Ordinary { .. })) {
                let name = vm.get_property(*v, "name").unwrap_or(Value::Undefined);
                let message = vm.get_property(*v, "message").unwrap_or(Value::Undefined);
                if !matches!(name, Value::Undefined) {
                    return format!("{}: {}", vm.format_value(name), vm.format_value(message));
                }
            }
        }
        return vm.format_value(*v);
    }
    format!("{err}")
}

// --- 实例方法 ---------------------------------------------------------------

/// `channel.subscribe(fn)`。
fn instance_subscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    if !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "diagnostics_channel: subscriber must be a function",
        ));
    }
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    channel_subscribe(vm, id, cb)?;
    Ok(Value::Undefined)
}

/// `channel.unsubscribe([fn])`。
fn instance_unsubscribe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Boolean(false));
    };
    let Some(cb) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let removed = channel_unsubscribe(vm, id, cb)?;
    Ok(Value::Boolean(removed))
}

/// `channel.publish([message])`：订阅者在调用方异步上下文执行。
fn instance_publish(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let message = args.first().copied().unwrap_or(Value::Undefined);
    channel_publish(vm, id, message)?;
    Ok(Value::Undefined)
}

/// `channel.bindStore(store)`：重绑移到末尾（对齐 Go 的追加语义）。
fn instance_bind_store(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if let Some(store) = args.first().copied() {
        channel_of(id, |st| {
            if let Some(pos) = st.stores.iter().position(|s| *s == store) {
                st.stores.remove(pos);
            }
            st.stores.push(store);
        });
    }
    Ok(Value::Undefined)
}

/// `channel.unbindStore(store)`。
fn instance_unbind_store(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    if let Some(store) = args.first().copied() {
        channel_of(id, |st| {
            if let Some(pos) = st.stores.iter().position(|s| *s == store) {
                st.stores.remove(pos);
            }
        });
    }
    Ok(Value::Undefined)
}

/// `channel.runStores(context, callback, ...args)`：逐层经 `store.run` 进入
/// 绑定上下文后调用回调。
fn instance_run_stores(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = receiver_id() else {
        return Ok(Value::Undefined);
    };
    let context = args.first().copied().unwrap_or(Value::Undefined);
    let cb = args.get(1).copied().unwrap_or(Value::Undefined);
    if args.len() < 2 || !is_function(vm, cb) {
        return Err(thrown(
            vm,
            "diagnostics_channel: runStores requires a callback",
        ));
    }
    let stores = channel_of(id, |st| st.stores.clone()).unwrap_or_default();
    let rest: Vec<Value> = args.iter().skip(2).copied().collect();
    chain_invoke(vm, &stores, context, cb, &rest)
}

/// 逐层包 `store.run(context, wrapped, ...args)`（可嵌套多个绑定 store）。
fn chain_invoke(
    vm: &mut Vm,
    stores: &[Value],
    context: Value,
    callback: Value,
    call_args: &[Value],
) -> Result<Value, VmError> {
    let Some(first) = stores.first().copied() else {
        return vm.invoke_callable(callback, Value::Undefined, call_args);
    };
    let run_fn = vm.get_property(first, "run").unwrap_or(Value::Undefined);
    if !is_function(vm, run_fn) {
        return chain_invoke(vm, &stores[1..], context, callback, call_args);
    }
    let wrapped = vm.alloc_native_fn("diagnostics_channel:runStores.chain");
    CHAIN.with(|g| {
        g.borrow_mut().push(ChainCtx {
            remaining: stores[1..].to_vec(),
            context,
            callback,
        });
    });
    let mut forwarded = vec![context, Value::Object(wrapped)];
    forwarded.extend_from_slice(call_args);
    vm.invoke_callable(run_fn, first, &forwarded)
}

/// `runStores` 链式续体：弹出最近一层上下文并继续进入剩余 store。
fn run_stores_chain(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ctx = CHAIN.with(|g| g.borrow_mut().pop());
    let Some(ctx) = ctx else {
        return Ok(Value::Undefined);
    };
    chain_invoke(vm, &ctx.remaining, ctx.context, ctx.callback, args)
}

/// 当前分派接收者的堆句柄 id。
fn receiver_id() -> Option<u32> {
    match current_receiver().case() {
        ValueCase::Object(r) => Some(r.0),
        _ => None,
    }
}

/// 当前分派接收者（tracing 聚合对象）的句柄 id。
fn current_tracing_id() -> Option<u32> {
    receiver_id()
}

/// 编译期锚定：确保处理器签名与注册表一致。
/// GC 根快照：通道实例对象、订阅回调与绑定 store。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    CHANNELS.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for c in map.values() {
                out.push(Value::Object(c.obj));
                for v in &c.subscribers {
                    out.push(*v);
                }
                for v in &c.stores {
                    out.push(*v);
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
        let _: crate::builtins::BuiltinHandler = module_channel;
        let _: crate::builtins::BuiltinHandler = module_has_subscribers;
        let _: crate::builtins::BuiltinHandler = module_subscribe;
        let _: crate::builtins::BuiltinHandler = module_unsubscribe;
        let _: crate::builtins::BuiltinHandler = module_tracing_channel;
        let _: crate::builtins::BuiltinHandler = channel_ctor;
        let _: crate::builtins::BuiltinHandler = instance_subscribe;
        let _: crate::builtins::BuiltinHandler = instance_unsubscribe;
        let _: crate::builtins::BuiltinHandler = instance_publish;
        let _: crate::builtins::BuiltinHandler = instance_bind_store;
        let _: crate::builtins::BuiltinHandler = instance_unbind_store;
        let _: crate::builtins::BuiltinHandler = instance_run_stores;
        let _: crate::builtins::BuiltinHandler = tracing_subscribe;
        let _: crate::builtins::BuiltinHandler = tracing_unsubscribe;
        let _: crate::builtins::BuiltinHandler = tracing_trace;
        let _: crate::builtins::BuiltinHandler = run_stores_chain;
    }
}
