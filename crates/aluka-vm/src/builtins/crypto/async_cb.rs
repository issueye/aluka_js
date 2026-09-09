//! crypto 异步回调投递：把 `pbkdf2` / `scrypt` / `hkdf` / `randomInt` /
//! `checkPrime` / `randomFill` 的回调排进宏任务队列，在事件循环宏任务阶段
//! 以 `(err)` 或 `(null, 结果)` 实参回放。
//!
//! 实现说明：宏任务泵按 `invoke_callable(cb, undefined, &[])` 无参调用回调，
//! 因此调度的是本模块注册的**投递蹦床**（`crypto.deliver.async`），投递载荷
//! 存放于线程局部 FIFO 队列，蹦床按序弹出并携带实参调用用户回调。宏任务按注册
//! 顺序执行 ⇒ FIFO 弹出与注册顺序严格一致。

use crate::builtins::buffer;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::cell::RefCell;
use std::collections::VecDeque;

/// 投递载荷：错误串或成功结果值。
#[derive(Debug)]
pub(crate) enum Delivery {
    /// 失败：回调以 `(错误消息串)` 单参调用
    Err(String),
    /// 成功：回调以 `(null, Buffer)` 调用
    Bytes(Vec<u8>),
    /// 成功：回调以 `(null, Number)` 调用
    Int(i64),
    /// 成功：回调以 `(null, Boolean)` 调用
    Bool(bool),
    /// 成功：回调以 `(null, 原值)` 调用（randomFill 透传原 Buffer）
    Passthrough(Value),
}

/// 投递条目：用户回调 + 载荷。
struct PendingDelivery {
    cb: Value,
    delivery: Delivery,
}

// 投递 FIFO 队列（线程局部：crypto 异步为同步模拟，载荷仅在主 Vm 线程排入与消费）。
thread_local! {
    static DELIVERY_QUEUE: RefCell<VecDeque<PendingDelivery>> = const { RefCell::new(VecDeque::new()) };
}

/// 登记一次异步投递并调度宏任务蹦床。
pub(crate) fn schedule_delivery(vm: &mut Vm, cb: Value, delivery: Delivery) {
    DELIVERY_QUEUE.with(|q| q.borrow_mut().push_back(PendingDelivery { cb, delivery }));
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    let trampoline = vm.alloc_native_fn("crypto.deliver.async");
    vm.macro_tasks
        .push_back((id, last_due, 0, Value::Object(trampoline), false));
}

/// 蹦床处理器：弹出队首载荷并实参回放用户回调。
fn deliver(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(PendingDelivery { cb, delivery }) =
        DELIVERY_QUEUE.with(|q| q.borrow_mut().pop_front())
    else {
        return Ok(Value::Undefined);
    };
    match delivery {
        Delivery::Err(msg) => {
            let err_val = Value::Object(vm.alloc_string(msg));
            vm.invoke_callable(cb, Value::Undefined, &[err_val])?;
        }
        Delivery::Bytes(bytes) => {
            let buf = buffer::create_buffer_instance(vm, bytes);
            vm.invoke_callable(cb, Value::Undefined, &[Value::Null, Value::Object(buf)])?;
        }
        Delivery::Int(v) => {
            vm.invoke_callable(
                cb,
                Value::Undefined,
                &[Value::Null, Value::Number(v as f64)],
            )?;
        }
        Delivery::Bool(v) => {
            vm.invoke_callable(cb, Value::Undefined, &[Value::Null, Value::Boolean(v)])?;
        }
        Delivery::Passthrough(v) => {
            vm.invoke_callable(cb, Value::Undefined, &[Value::Null, v])?;
        }
    }
    Ok(Value::Undefined)
}

/// 在模块 build 时登记蹦床分派键。
pub(crate) fn register(registry: &mut crate::builtins::BuiltinRegistry) {
    crate::builtins::register_handler(registry, "crypto", "deliver.async", deliver);
}

/// 值是否为可调用对象（闭包 / 原生函数 / 原生构造器）。
pub(crate) fn is_callable(vm: &Vm, v: Value) -> bool {
    matches!(
        v,
        Value::Object(r) if matches!(
            vm.heap.get(r.0 as usize),
            Some(
                crate::heap::HeapObject::Closure { .. }
                | crate::heap::HeapObject::NativeFn { .. }
                | crate::heap::HeapObject::NativeCtor { .. }
            )
        )
    )
}

/// 构造 JS 异常（Error 实例，`.message` 可读，对齐 Node.js 22 LTS 标准 抛错形态）。
pub(crate) fn throw_error(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}

/// GC 根快照：待投递异步回调（漏登记 = 高频回收下回调悬垂）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    DELIVERY_QUEUE.with(|q| {
        for d in q.borrow().iter() {
            out.push(d.cb);
        }
    });
}
