//! `timers` 与 `timers/promises` 内置模块（Phase 3）：Node 定时器与 Promise 化接口。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodetimers`）：
//! - `timers`：`setTimeout` / `clearTimeout` / `setInterval` / `clearInterval` / `setImmediate` / `clearImmediate`；
//! - `timers/promises`：`setTimeout(delay, [value]) -> Promise<value>`、`setImmediate([value]) -> Promise<value>`，可直接供 async 函数 `await`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::sync::Mutex;

static RESOLVER_VALS: Mutex<Option<HashMap<u32, Value>>> = Mutex::new(None);

/// GC root provider：解析器预存兑现值（timers/promises 延迟兑现载体）。
pub(crate) fn resolver_roots(out: &mut crate::gc::GcRoots) {
    let guard = RESOLVER_VALS.lock().unwrap();
    if let Some(map) = guard.as_ref() {
        for v in map.values() {
            out.push(*v);
        }
    }
}

/// 暂存 PromiseResolver 对应的预设兑现值。
pub fn set_resolver_val(id: u32, val: Value) {
    let mut guard = RESOLVER_VALS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(id, val);
}

/// 取出并移除 PromiseResolver 对应的预设兑现值。
pub fn take_resolver_val(id: u32) -> Option<Value> {
    let mut guard = RESOLVER_VALS.lock().unwrap();
    guard.as_mut()?.remove(&id)
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
    register_handler(registry, "timers", "clearInterval", clear_timeout);
    register_handler(registry, "timers", "setImmediate", set_immediate);
    register_handler(registry, "timers", "clearImmediate", clear_timeout);

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
    schedule_timer(vm, args, false)
}

/// `timers.setInterval(cb, [delay])`
fn set_interval(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    schedule_timer(vm, args, true)
}

/// `timers.setImmediate(cb)`
fn set_immediate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    schedule_raw(vm, cb, 0, false)
}

/// `timers.clearTimeout(id)` / `clearInterval`
fn clear_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let id = args
        .first()
        .and_then(|v| match v {
            Value::Number(n) => Some(*n as u64),
            _ => None,
        })
        .unwrap_or(0);
    vm.active_timers.insert(id);
    Ok(Value::Undefined)
}

fn schedule_timer(vm: &mut Vm, args: &[Value], repeating: bool) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    let delay = args
        .get(1)
        .and_then(|v| match v {
            Value::Number(n) => Some((*n as i64).max(0) as u64),
            _ => None,
        })
        .unwrap_or(0);
    schedule_raw(vm, cb, delay, repeating)
}

fn schedule_raw(vm: &mut Vm, cb: Value, delay: u64, repeating: bool) -> Result<Value, VmError> {
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    let due = last_due + delay;
    vm.macro_tasks.push_back((id, due, delay, cb, repeating));
    Ok(Value::Number(id as f64))
}

/// `timers/promises.setTimeout([delay, value])`
fn promises_set_timeout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let delay = args
        .first()
        .and_then(|v| match v {
            Value::Number(n) => Some((*n as i64).max(0) as u64),
            _ => None,
        })
        .unwrap_or(0);

    let val = args.get(1).copied().unwrap_or(Value::Undefined);
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);

    set_resolver_val(resolver.0, val);

    schedule_raw(vm, Value::Object(resolver), delay, false)?;

    Ok(Value::Object(promise))
}

/// `timers/promises.setImmediate([value])`
fn promises_set_immediate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let val = args.first().copied().unwrap_or(Value::Undefined);
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);

    set_resolver_val(resolver.0, val);

    schedule_raw(vm, Value::Object(resolver), 0, false)?;

    Ok(Value::Object(promise))
}
