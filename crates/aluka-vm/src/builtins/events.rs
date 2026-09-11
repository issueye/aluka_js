//! `events` 内置模块（Phase 4）：Node EventEmitter 事件机制。
//!
//! 提供 `node:events` 模块导出与 EventEmitter 事件机制实现：
//! - 模块级导出：`EventEmitter`（类构造器函数）、`defaultMaxListeners`、`on`、`once`、`listenerCount`、`setMaxListeners`、`getMaxListeners`；
//! - `EventEmitter` 类静态方法：`listenerCount`、`on`、`once`、`setMaxListeners`、`getMaxListeners`；
//! - `EventEmitter` 实例方法：`on` / `addListener`、`once`、`emit`、`off` / `removeListener`、`removeAllListeners`、`listenerCount`、`setMaxListeners`、`getMaxListeners`、`prependListener`、`prependOnceListener`、`eventNames`、`listeners`、`rawListeners`。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// 监听器条目定义
#[derive(Debug, Clone)]
pub struct ListenerItem {
    /// 监听器回调函数引用
    pub callback: Value,
    /// 是否为单次触发监听器（once 注册）
    pub once: bool,
}

/// EventEmitter 实例内部事件状态
#[derive(Debug, Clone)]
pub struct EmitterState {
    /// 事件名到监听器列表映射表
    pub listeners: HashMap<String, Vec<ListenerItem>>,
    /// 当前实例的最大监听器限制数
    pub max_listeners: usize,
}

impl Default for EmitterState {
    fn default() -> Self {
        Self {
            listeners: HashMap::new(),
            max_listeners: 10,
        }
    }
}

// EventEmitter 状态存储（对象堆句柄索引 -> 实例状态；线程局部：堆句柄仅本线程有效）
thread_local! {
    static EMITTER_STORE: RefCell<Option<HashMap<u32, EmitterState>>> = const { RefCell::new(None) };
}

/// 保存指定实例的事件状态
fn store_emitter(id: u32, state: EmitterState) {
    EMITTER_STORE.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, state);
    });
}

/// 可变借用执行闭包访问指定实例的状态
fn with_emitter_mut<F, R>(id: u32, f: F) -> R
where
    F: FnOnce(&mut EmitterState) -> R,
{
    EMITTER_STORE.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        let state = map.entry(id).or_default();
        f(state)
    })
}

/// 只读借用执行闭包访问指定实例的状态
fn with_emitter<F, R>(id: u32, f: F) -> R
where
    F: FnOnce(&EmitterState) -> R,
{
    EMITTER_STORE.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        let state = map.entry(id).or_default();
        f(state)
    })
}

/// 确保指定实例状态已在全局映射中就绪（支持堆中历史 EventEmitter 对象状态同步）
fn ensure_emitter_state(vm: &Vm, id: u32) {
    EMITTER_STORE.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        map.entry(id).or_insert_with(|| {
            let mut state = EmitterState::default();
            if let Some(HeapObject::EventEmitter { listeners }) = vm.heap.get(id as usize) {
                for (k, list) in listeners {
                    let items: Vec<ListenerItem> = list
                        .iter()
                        .map(|(cb, once)| ListenerItem {
                            callback: *cb,
                            once: *once,
                        })
                        .collect();
                    state.listeners.insert(k.clone(), items);
                }
            }
            state
        });
    });
}

/// 创建全新的 EventEmitter 实例对象并初始化属性和方法
pub fn create_emitter_instance(vm: &mut Vm) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    store_emitter(obj.0, EmitterState::default());

    let _ = vm.set_property(Value::Object(obj), "_isEventEmitter", Value::Boolean(true));

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
        let fn_ref = vm.alloc_native_fn(&format!("events:instance.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }

    obj
}

/// `require("events")` / `require("node:events")` 核心模块导出条目
pub const MODULE: ModuleDef = ModuleDef {
    name: "events",
    build,
};

/// `EventEmitter` 类对象导出条目（支持静态方法分派）
pub const EMITTER_CLASS_MODULE: ModuleDef = ModuleDef {
    name: "EventEmitter",
    build: build_class,
};

/// `events:instance` 实例方法槽位模块条目
pub const INSTANCE_MODULE: ModuleDef = ModuleDef {
    name: "events:instance",
    build: build_instance,
};

/// 构建 `events` 核心模块（Node 规范中模块本身即为 EventEmitter 构造器）
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let ee_class = vm.alloc_native_ctor("EventEmitter", None);

    // 设置默认最大监听器数量属性
    set_module_prop(vm, ee_class, "defaultMaxListeners", Value::Number(10.0))?;

    // 循环引用导出：events.EventEmitter === events
    set_module_prop(vm, ee_class, "EventEmitter", Value::Object(ee_class))?;

    // 实例原型对象
    let proto = vm.alloc_ordinary();
    set_module_prop(vm, proto, "constructor", Value::Object(ee_class))?;
    set_module_prop(vm, proto, "_isEventEmitter", Value::Boolean(true))?;
    set_module_prop(vm, ee_class, "prototype", Value::Object(proto))?;

    // 在 prototype 原型对象上挂载实例方法原生函数引用
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
        let fn_ref = vm.alloc_native_fn(&format!("events:instance.{method}"));
        set_module_prop(vm, proto, method, Value::Object(fn_ref))?;
    }

    // 在模块/构造器对象上挂载静态方法函数引用
    for method in [
        "on",
        "once",
        "listenerCount",
        "setMaxListeners",
        "getMaxListeners",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("events.{method}"));
        set_module_prop(vm, ee_class, method, Value::Object(fn_ref))?;
    }

    // 注册静态方法处理器
    register_handler(registry, "events", "on", events_static_on);
    register_handler(registry, "events", "once", events_static_once);
    register_handler(
        registry,
        "events",
        "listenerCount",
        events_static_listener_count,
    );
    register_handler(
        registry,
        "events",
        "setMaxListeners",
        events_static_set_max_listeners,
    );
    register_handler(
        registry,
        "events",
        "getMaxListeners",
        events_static_get_max_listeners,
    );

    // 同步解释器单例句柄
    vm.events_module = Some(ee_class);

    Ok(ee_class)
}

/// 构建 `EventEmitter` 类模块
fn build_class(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let events_mod = registry.module("events").ok_or_else(|| {
        let msg = vm.alloc_string("events 模块尚未初始化".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;

    // 在 EventEmitter 静态键名下注册分派处理器
    for method in [
        "on",
        "once",
        "listenerCount",
        "setMaxListeners",
        "getMaxListeners",
    ] {
        if let Some(h) = registry.lookup(&format!("events.{method}")) {
            register_handler(registry, "EventEmitter", method, h);
        }
    }

    Ok(events_mod)
}

/// 构建 `events:instance` 实例方法槽位模块
fn build_instance(_vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let slot = ObjectRef(0);
    register_handler(registry, "events:instance", "on", emitter_on);
    register_handler(registry, "events:instance", "addListener", emitter_on);
    register_handler(registry, "events:instance", "once", emitter_once);
    register_handler(registry, "events:instance", "emit", emitter_emit);
    register_handler(registry, "events:instance", "off", emitter_off);
    register_handler(registry, "events:instance", "removeListener", emitter_off);
    register_handler(
        registry,
        "events:instance",
        "removeAllListeners",
        emitter_remove_all_listeners,
    );
    register_handler(
        registry,
        "events:instance",
        "listenerCount",
        emitter_listener_count,
    );
    register_handler(
        registry,
        "events:instance",
        "setMaxListeners",
        emitter_set_max_listeners,
    );
    register_handler(
        registry,
        "events:instance",
        "getMaxListeners",
        emitter_get_max_listeners,
    );
    register_handler(
        registry,
        "events:instance",
        "prependListener",
        emitter_prepend_listener,
    );
    register_handler(
        registry,
        "events:instance",
        "prependOnceListener",
        emitter_prepend_once_listener,
    );
    register_handler(
        registry,
        "events:instance",
        "eventNames",
        emitter_event_names,
    );
    register_handler(registry, "events:instance", "listeners", emitter_listeners);
    register_handler(
        registry,
        "events:instance",
        "rawListeners",
        emitter_listeners,
    );
    Ok(slot)
}

/// `emitter.on(event, listener)` / `emitter.addListener(event, listener)`
pub(crate) fn emitter_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() < 2 {
        return Ok(receiver);
    }
    let event_name = vm.format_value(args[0]);
    let callback = args[1];

    with_emitter_mut(r.0, |state| {
        state
            .listeners
            .entry(event_name)
            .or_default()
            .push(ListenerItem {
                callback,
                once: false,
            });
    });

    Ok(receiver)
}

/// `emitter.once(event, listener)`
fn emitter_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() < 2 {
        return Ok(receiver);
    }
    let event_name = vm.format_value(args[0]);
    let callback = args[1];

    with_emitter_mut(r.0, |state| {
        state
            .listeners
            .entry(event_name)
            .or_default()
            .push(ListenerItem {
                callback,
                once: true,
            });
    });

    Ok(receiver)
}

/// `emitter.emit(event, ...args)`
pub(crate) fn emitter_emit(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Boolean(false));
    };
    ensure_emitter_state(vm, r.0);

    let Some(event_val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let event_name = vm.format_value(*event_val);
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();

    // 收集待触发监听器快照，并将一次性监听器移除
    let listeners_to_call: Vec<Value> = with_emitter_mut(r.0, |state| {
        let Some(list) = state.listeners.get_mut(&event_name) else {
            return Vec::new();
        };
        let mut to_call = Vec::new();
        let mut keep = Vec::with_capacity(list.len());
        for item in list.drain(..) {
            to_call.push(item.callback);
            if !item.once {
                keep.push(item);
            }
        }
        *list = keep;
        to_call
    });

    if listeners_to_call.is_empty() {
        if event_name == "error" {
            let err_val = emit_args.first().copied().unwrap_or(Value::Undefined);
            return Err(VmError::Thrown(err_val));
        }
        return Ok(Value::Boolean(false));
    }

    for cb in listeners_to_call {
        let _ = vm.invoke_callable(cb, receiver, &emit_args)?;
    }

    Ok(Value::Boolean(true))
}

/// `emitter.off(event, listener)` / `emitter.removeListener(event, listener)`
fn emitter_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() < 2 {
        return Ok(receiver);
    }
    let event_name = vm.format_value(args[0]);
    let target_cb = args[1];

    with_emitter_mut(r.0, |state| {
        if let Some(list) = state.listeners.get_mut(&event_name) {
            if let Some(pos) = list.iter().position(|item| item.callback == target_cb) {
                list.remove(pos);
            }
        }
    });

    Ok(receiver)
}

/// `emitter.removeAllListeners([event])`
fn emitter_remove_all_listeners(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    with_emitter_mut(r.0, |state| match args.first().map(|v| v.case()) {
        Some(ValueCase::Undefined) | None => {
            state.listeners.clear();
        }
        Some(v) => {
            let name = vm.format_value(Value::from(v));
            state.listeners.remove(&name);
        }
    });

    Ok(receiver)
}

/// `emitter.listenerCount(event)`
fn emitter_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Number(0.0));
    };
    ensure_emitter_state(vm, r.0);

    let Some(event_val) = args.first() else {
        return Ok(Value::Number(0.0));
    };
    let event_name = vm.format_value(*event_val);

    let count = with_emitter(r.0, |state| {
        state
            .listeners
            .get(&event_name)
            .map(|l| l.len())
            .unwrap_or(0)
    });

    Ok(Value::Number(count as f64))
}

/// `emitter.setMaxListeners(n)`
fn emitter_set_max_listeners(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };

    let n = match args.first().map(|v| v.case()) {
        Some(ValueCase::Number(num)) => num as usize,
        _ => 10,
    };

    with_emitter_mut(r.0, |state| {
        state.max_listeners = n;
    });

    Ok(receiver)
}

/// `emitter.getMaxListeners()`
fn emitter_get_max_listeners(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Number(10.0));
    };

    let max = with_emitter(r.0, |state| state.max_listeners);
    Ok(Value::Number(max as f64))
}

/// `emitter.prependListener(event, listener)`
fn emitter_prepend_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() < 2 {
        return Ok(receiver);
    }
    let event_name = vm.format_value(args[0]);
    let callback = args[1];

    with_emitter_mut(r.0, |state| {
        state.listeners.entry(event_name).or_default().insert(
            0,
            ListenerItem {
                callback,
                once: false,
            },
        );
    });

    Ok(receiver)
}

/// `emitter.prependOnceListener(event, listener)`
fn emitter_prepend_once_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() < 2 {
        return Ok(receiver);
    }
    let event_name = vm.format_value(args[0]);
    let callback = args[1];

    with_emitter_mut(r.0, |state| {
        state.listeners.entry(event_name).or_default().insert(
            0,
            ListenerItem {
                callback,
                once: true,
            },
        );
    });

    Ok(receiver)
}

/// `emitter.eventNames()`
fn emitter_event_names(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    ensure_emitter_state(vm, r.0);

    let names: Vec<Value> = with_emitter(r.0, |state| {
        state
            .listeners
            .iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(k, _)| {
                let s_ref = vm.alloc_string(k.clone());
                Value::Object(s_ref)
            })
            .collect()
    });

    Ok(Value::Object(vm.alloc_array(names)))
}

/// `emitter.listeners(event)`
fn emitter_listeners(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    ensure_emitter_state(vm, r.0);

    let Some(event_val) = args.first() else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    let event_name = vm.format_value(*event_val);

    let list: Vec<Value> = with_emitter(r.0, |state| {
        state
            .listeners
            .get(&event_name)
            .map(|l| l.iter().map(|item| item.callback).collect())
            .unwrap_or_default()
    });

    Ok(Value::Object(vm.alloc_array(list)))
}

/// `events.listenerCount(emitter, event)` / `EventEmitter.listenerCount(emitter, event)`
fn events_static_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(emitter_val) = args.first() else {
        return Ok(Value::Number(0.0));
    };
    let Some(event_val) = args.get(1) else {
        return Ok(Value::Number(0.0));
    };
    let ValueCase::Object(r) = emitter_val.case() else {
        return Ok(Value::Number(0.0));
    };
    ensure_emitter_state(vm, r.0);

    let event_name = vm.format_value(*event_val);
    let count = with_emitter(r.0, |state| {
        state
            .listeners
            .get(&event_name)
            .map(|l| l.len())
            .unwrap_or(0)
    });
    Ok(Value::Number(count as f64))
}

/// `events.on(emitter, event, listener)`
fn events_static_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(emitter_val) = args.first() else {
        return Ok(Value::Undefined);
    };
    let ValueCase::Object(r) = emitter_val.case() else {
        return Ok(Value::Undefined);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() >= 3 {
        let event_name = vm.format_value(args[1]);
        let callback = args[2];
        with_emitter_mut(r.0, |state| {
            state
                .listeners
                .entry(event_name)
                .or_default()
                .push(ListenerItem {
                    callback,
                    once: false,
                });
        });
        return Ok(*emitter_val);
    }
    Ok(Value::Undefined)
}

/// `events.once(emitter, event, listener)`
fn events_static_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(emitter_val) = args.first() else {
        return Ok(Value::Undefined);
    };
    let ValueCase::Object(r) = emitter_val.case() else {
        return Ok(Value::Undefined);
    };
    ensure_emitter_state(vm, r.0);

    if args.len() >= 3 {
        let event_name = vm.format_value(args[1]);
        let callback = args[2];
        with_emitter_mut(r.0, |state| {
            state
                .listeners
                .entry(event_name)
                .or_default()
                .push(ListenerItem {
                    callback,
                    once: true,
                });
        });
        return Ok(*emitter_val);
    }
    Ok(Value::Undefined)
}

/// `events.setMaxListeners(n, ...emitters)`
fn events_static_set_max_listeners(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(n) = args.first().and_then(|v| v.as_number()) {
        let limit = n as usize;
        for target in args.iter().skip(1) {
            if let Some(r) = target.as_object() {
                with_emitter_mut(r.0, |state| {
                    state.max_listeners = limit;
                });
            }
        }
    }
    Ok(Value::Undefined)
}

/// `events.getMaxListeners(emitter)`
fn events_static_get_max_listeners(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(r) = args.first().and_then(|v| v.as_object()) else {
        return Ok(Value::Number(10.0));
    };
    let max = with_emitter(r.0, |state| state.max_listeners);
    Ok(Value::Number(max as f64))
}

/// GC 根快照：EventEmitter 侧表监听器回调。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    EMITTER_STORE.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for state in map.values() {
                for items in state.listeners.values() {
                    for item in items {
                        out.push(item.callback);
                    }
                }
            }
        }
    });
}
