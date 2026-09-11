//! `BroadcastChannel` 全局类（node22 conformance 06）：同频道多实例广播。
//!
//! 语义对齐 Node：
//! - `new BroadcastChannel(name)`：加入同名频道；实例是 EventEmitter
//!   （`on`/`addEventListener` 监听 `'message'`）；
//! - `postMessage(msg)`：**其它**同频道实例收到 `message` 事件（参数
//!   `{ data }`），自身不收到；
//! - `close()`：退出频道，此后 postMessage 为 no-op。
//!
//! 实例复用 EventEmitter 基建（监听器走 events 模块存储）；频道路由表为
//! 线程局部（与 STREAM_STORE 同款模式）。

use crate::VmError;
use crate::builtins::events::{emitter_emit, emitter_on};
use crate::builtins::{current_receiver, set_current_receiver};
use crate::interpreter::Vm;
use crate::value::{Value, ValueCase};
use std::cell::RefCell;
use std::collections::HashMap;

// 频道路由表：频道名 → 成员实例句柄（按加入顺序）。
thread_local! {
    // CHANNELS：线程局部（堆句柄仅本线程 Vm 有效）。
    static CHANNELS: RefCell<Option<HashMap<String, Vec<u32>>>> = const { RefCell::new(None) };
}

fn with_channels<R>(f: impl FnOnce(&mut HashMap<String, Vec<u32>>) -> R) -> R {
    CHANNELS.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// 频道名（receiver 的 `name` 属性）。
fn channel_name_of(vm: &mut Vm, receiver: Value) -> String {
    vm.get_property(receiver, "name")
        .ok()
        .map(|v| vm.format_value(v))
        .unwrap_or_default()
}

/// `new BroadcastChannel(name)`：EventEmitter 实例 + 频道注册。
fn bc_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let inst = crate::builtins::events::create_emitter_instance(vm);
    let _ = vm.set_property(
        Value::Object(inst),
        "_isBroadcastChannel",
        Value::Boolean(true),
    );
    let name_str = vm.alloc_string(name.clone());
    let _ = vm.set_property(Value::Object(inst), "name", Value::Object(name_str));

    let post_ref = vm.alloc_native_fn("broadcast_channel.postMessage");
    let close_ref = vm.alloc_native_fn("broadcast_channel.close");
    let on_ref = vm.alloc_native_fn("broadcast_channel.on");
    let _ = vm.set_property(Value::Object(inst), "postMessage", Value::Object(post_ref));
    let _ = vm.set_property(Value::Object(inst), "close", Value::Object(close_ref));
    let _ = vm.set_property(Value::Object(inst), "on", Value::Object(on_ref));
    let _ = vm.set_property(
        Value::Object(inst),
        "addEventListener",
        Value::Object(on_ref),
    );

    with_channels(|ch| {
        ch.entry(name).or_default().push(inst.0);
    });
    Ok(Value::Object(inst))
}

/// `postMessage(msg)`：广播给同频道**其它**实例（自身不回环），
/// 事件参数为 `{ data }`。
fn bc_post_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let self_id = match receiver.case() {
        ValueCase::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let name = channel_name_of(vm, receiver);
    let msg = args.first().copied().unwrap_or(Value::Undefined);

    // 事件参数 { data }
    let event = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(event), "data", msg);
    let ev_name = vm.alloc_string("message".to_owned());

    let targets: Vec<u32> = with_channels(|ch| {
        ch.get(&name)
            .map(|members| {
                members
                    .iter()
                    .copied()
                    .filter(|id| *id != self_id)
                    .collect()
            })
            .unwrap_or_default()
    });
    for id in targets {
        let member = Value::Object(aluka_core::ObjectRef(id));
        set_current_receiver(member);
        // emitter_emit 约定：args[0] 为事件名，其后为事件参数
        emitter_emit(vm, &[Value::Object(ev_name), Value::Object(event)])?;
    }
    Ok(Value::Undefined)
}

/// `close()`：退出频道（此后 postMessage 不再广播到本实例）。
fn bc_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(r) = receiver.as_object() {
        let name = channel_name_of(vm, receiver);
        with_channels(|ch| {
            if let Some(members) = ch.get_mut(&name) {
                members.retain(|id| *id != r.0);
            }
        });
    }
    Ok(Value::Undefined)
}

/// `on` / `addEventListener`：委托 EventEmitter 监听器注册。
fn bc_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    emitter_on(vm, args)
}

/// 注册 BroadcastChannel 全局类（`Vm::new` 内置装配时调用一次）。
pub fn register_global(vm: &mut Vm) {
    let ctor = vm.alloc_native_fn("global.BroadcastChannel");
    vm.globals
        .insert("BroadcastChannel".to_owned(), Value::Object(ctor));
    crate::builtins::register_handler(
        &mut vm.builtin_registry,
        "global",
        "BroadcastChannel",
        bc_ctor,
    );
    crate::builtins::register_handler(
        &mut vm.builtin_registry,
        "broadcast_channel",
        "postMessage",
        bc_post_message,
    );
    crate::builtins::register_handler(
        &mut vm.builtin_registry,
        "broadcast_channel",
        "close",
        bc_close,
    );
    crate::builtins::register_handler(&mut vm.builtin_registry, "broadcast_channel", "on", bc_on);
}
