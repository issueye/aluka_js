//! `timers` 与 `timers/promises` 内置模块（Phase 3）：Node 定时器与 Promise 化接口。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodetimers`）：
//! - `timers`：`setTimeout` / `clearTimeout` / `setInterval` / `clearInterval` / `setImmediate` / `clearImmediate`；
//! - `timers/promises`：`setTimeout(delay, [value]) -> Promise<value>`、`setImmediate([value]) -> Promise<value>`，可直接供 async 函数 `await`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
// 假时钟（`node:test` 的 `mock.timers`）拦截：定时器注册/清除的唯一入口。
use crate::builtins::test::mock;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

// 解析器预存兑现值（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static RESOLVER_VALS: RefCell<Option<HashMap<u32, Value>>> = const { RefCell::new(None) };
}

/// GC root provider：解析器预存兑现值（timers/promises 延迟兑现载体）。
pub(crate) fn resolver_roots(out: &mut crate::gc::GcRoots) {
    let vals: Vec<Value> = RESOLVER_VALS.with(|g| {
        g.borrow()
            .as_ref()
            .map(|m| m.values().copied().collect())
            .unwrap_or_default()
    });
    for v in vals {
        out.push(v);
    }
}

/// 暂存 PromiseResolver 对应的预设兑现值。
pub fn set_resolver_val(id: u32, val: Value) {
    RESOLVER_VALS.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, val);
    });
}

/// 取出并移除 PromiseResolver 对应的预设兑现值。
pub fn take_resolver_val(id: u32) -> Option<Value> {
    RESOLVER_VALS.with(|g| g.borrow_mut().as_mut()?.remove(&id))
}

/// `require("timers")` / `require("node:timers")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "timers",
    build,
};

/// `require("timers/promises")` / `require("node:timers/promises")` 子模块。
pub const PROMISES_MODULE: ModuleDef = ModuleDef {
    name: "timers/promises",
    build: build_promises,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in [
        "setTimeout",
        "clearTimeout",
        "setInterval",
        "clearInterval",
        "setImmediate",
        "clearImmediate",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("timers.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }

    register_handler(registry, "timers", "setTimeout", set_timeout);
    register_handler(registry, "timers", "clearTimeout", clear_timeout);
    register_handler(registry, "timers", "setInterval", set_interval);
    register_handler(registry, "timers", "clearInterval", clear_interval);
    register_handler(registry, "timers", "setImmediate", set_immediate);
    register_handler(registry, "timers", "clearImmediate", clear_immediate);
    // M4.3：AbortSignal 'abort' → 定时器清除（signal 联动内部通道）
    register_handler(registry, "timers", "signalClear", timers_signal_clear);

    Ok(obj)
}

fn build_promises(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in ["setTimeout", "setImmediate"] {
        let fn_ref = vm.alloc_native_fn(&format!("timers/promises.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }

    register_handler(
        registry,
        "timers/promises",
        "setTimeout",
        promises_set_timeout,
    );
    register_handler(
        registry,
        "timers/promises",
        "setImmediate",
        promises_set_immediate,
    );

    Ok(obj)
}

/// `timers.setTimeout(cb, [delay])`
fn set_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    schedule_timer(vm, args, mock::FakeApi::SetTimeout)
}

/// `timers.setInterval(cb, [delay])`
fn set_interval(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    schedule_timer(vm, args, mock::FakeApi::SetInterval)
}

/// `timers.setImmediate(cb[, options])`：options.signal 联动（M4.3）。
fn set_immediate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    let id_val = schedule_raw(vm, cb, 0, mock::FakeApi::SetImmediate)?;
    if let Some(opts) = args.get(1) {
        if let Some(id) = id_val.as_number() {
            attach_timer_signal(vm, id as u64, opts)?;
        }
    }
    Ok(id_val)
}

/// 定时器 → options.signal 联动（M4.3）：已 abort 立即清除；
/// 未 abort 挂 'abort' 监听（触发即等价 clearTimeout）。
fn attach_timer_signal(vm: &mut Vm, timer_id: u64, opts: &Value) -> Result<(), VmError> {
    let Ok(signal) = vm.get_property(*opts, "signal") else {
        return Ok(());
    };
    if matches!(signal, Value::Undefined | Value::Null) {
        return Ok(());
    }
    // 已 abort → 直接清除
    if let Ok(ValueCase::Boolean(true)) = vm.get_property(signal, "aborted").map(ValueCase::from) {
        vm.active_timers.insert(timer_id);
        return Ok(());
    }
    // 未 abort：挂 'abort' 监听器（timers.signalClear 携带 timer_id）
    let clear_fn = vm.alloc_native_fn("timers.signalClear");
    let _ = vm.set_property(
        Value::Object(clear_fn),
        "_timerId",
        Value::Number(timer_id as f64),
    );
    let add = vm.alloc_native_fn("AbortSignal.addEventListener");
    let ev = vm.alloc_string("abort".to_owned());
    let _ = vm.invoke_callable(
        Value::Object(add),
        signal,
        &[Value::Object(ev), Value::Object(clear_fn)],
    );
    Ok(())
}

/// `timers.signalClear`（M4.3 内部）：AbortSignal 'abort' 触发时清除定时器。
/// receiver 为携带 `_timerId` 的 NativeFn——按 id 等价 clearTimeout。
fn timers_signal_clear(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    if let Ok(ValueCase::Number(n)) = vm.get_property(receiver, "_timerId").map(ValueCase::from) {
        vm.active_timers.insert(n as u64);
    }
    Ok(Value::Undefined)
}

/// `timers.clearTimeout(id)`
fn clear_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    clear_timer(vm, args, mock::FakeApi::SetTimeout)
}

/// `timers.clearInterval(id)`
fn clear_interval(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    clear_timer(vm, args, mock::FakeApi::SetInterval)
}

/// `timers.clearImmediate(id)`
fn clear_immediate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    clear_timer(vm, args, mock::FakeApi::SetImmediate)
}

/// `timers.clearTimeout/clearInterval/clearImmediate` 共用实现。
///
/// 假时钟接管时（见 [`mock::fake_clear`]）清除请求只作用于假队列——Node
/// `MockTimers.#clearTimer` 只认假句柄，传入真实句柄是 no-op。
fn clear_timer(vm: &mut Vm, args: &[Value], api: mock::FakeApi) -> Result<Value, VmError> {
    let id = args
        .first()
        .and_then(|v| match v.case() {
            ValueCase::Number(n) => Some(n as u64),
            _ => None,
        })
        .unwrap_or(0);
    if mock::fake_clear(id, api) {
        return Ok(Value::Undefined);
    }
    vm.active_timers.insert(id);
    Ok(Value::Undefined)
}

fn schedule_timer(vm: &mut Vm, args: &[Value], api: mock::FakeApi) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    let delay = args
        .get(1)
        .and_then(|v| match v.case() {
            ValueCase::Number(n) => Some((n as i64).max(0) as u64),
            _ => None,
        })
        .unwrap_or(0);
    let id_val = schedule_raw(vm, cb, delay, api)?;
    // M4.3：第三参 options.signal 联动
    if let (Some(opts), ValueCase::Number(id)) = (args.get(2), id_val) {
        attach_timer_signal(vm, id as u64, opts)?;
    }
    Ok(id_val)
}

/// 定时器入队（真实宏任务队列）。
///
/// **假时钟先行拦截**（M5.4 切片二）：`mock.timers.enable({apis})` 覆盖该 api 时，
/// 定时器只登记进假时钟队列，**不写 `macro_tasks`**——既不真调度、也不走
/// `wait_until_due` 的真 `sleep`；`None` 时保持原有真实定时器行为。
/// crate 内复用：`worker_threads.postMessageToThread` 的 timeout 定时器
/// 走同一调度通路（假时钟拦截 + 真实宏任务登记）。
pub(crate) fn schedule_raw(
    vm: &mut Vm,
    cb: Value,
    delay: u64,
    api: mock::FakeApi,
) -> Result<Value, VmError> {
    if let Some(id) = mock::fake_schedule(cb, delay, api) {
        return Ok(id);
    }
    let repeating = matches!(api, mock::FakeApi::SetInterval);
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    let due = last_due + delay;
    vm.macro_tasks.push_back((id, due, delay, cb, repeating));
    Ok(Value::Number(id as f64))
}

/// `timers/promises.setTimeout([delay, value[, options]])`（M4.3：signal
/// abort → 挂起 promise 以 reason 拒绝，定时器同步清除）。
fn promises_set_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let delay = args
        .first()
        .and_then(|v| match v.case() {
            ValueCase::Number(n) => Some((n as i64).max(0) as u64),
            _ => None,
        })
        .unwrap_or(0);

    let val = args.get(1).copied().unwrap_or(Value::Undefined);
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);

    set_resolver_val(resolver.0, val);

    let id_val = schedule_raw(
        vm,
        Value::Object(resolver),
        delay,
        mock::FakeApi::SetTimeout,
    )?;

    // M4.3：options.signal——abort → 清除定时器 + promise 兑现 reason
    //（Node 语义：reason 缺省 AbortError；reject 与 resolve 经引擎同形
    // 兑现通道——非 undefined 值即拒绝近似）
    if let (Some(opts), ValueCase::Number(id)) = (args.get(2), id_val) {
        if let Ok(signal) = vm.get_property(*opts, "signal") {
            if !matches!(signal, Value::Undefined | Value::Null).case().map(ValueCase::from) {
                if let Ok(ValueCase::Boolean(true)) = vm.get_property(signal, "aborted").map(ValueCase::from) {
                    vm.active_timers.insert(id as u64);
                    let reason = vm
                        .get_property(signal, "reason")
                        .ok()
                        .filter(|v| !matches!(*v, Value::Undefined))
                        .unwrap_or_else(|| {
                            let err = vm.alloc_error_instance("This operation was aborted");
                            let name = vm.alloc_string("AbortError".to_owned());
                            let _ =
                                vm.set_property(Value::Object(err), "name", Value::Object(name));
                            Value::Object(err)
                        });
                    let reject = vm.alloc_promise_resolver(promise, false);
                    let _ = vm.invoke_callable(Value::Object(reject), Value::Undefined, &[reason]);
                }
            }
        }
    }

    Ok(Value::Object(promise))
}

/// `timers/promises.setImmediate([value])`
/// （假时钟启用 `setImmediate` 时经 [`schedule_raw`] 拦截，fired 即兑现 promise）
fn promises_set_immediate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let val = args.first().copied().unwrap_or(Value::Undefined);
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);

    set_resolver_val(resolver.0, val);

    schedule_raw(vm, Value::Object(resolver), 0, mock::FakeApi::SetImmediate)?;

    Ok(Value::Object(promise))
}
