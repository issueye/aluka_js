//! AbortController / AbortSignal 全局构造器与实例方法。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

static ABORT_ID_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

pub(crate) fn next_abort_id() -> u32 {
    ABORT_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

pub(crate) fn alloc_abort_signal(vm: &mut Vm) -> Result<ObjectRef, VmError> {
    let abort_id = next_abort_id();
    let signal = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(signal), "aborted", Value::Boolean(false));
    let _ = vm.set_property(
        Value::Object(signal),
        "_abortId",
        Value::Number(abort_id as f64),
    );
    let _ = vm.set_property(
        Value::Object(signal),
        "_isAbortSignal",
        Value::Boolean(true),
    );
    let listeners = vm.alloc_array(Vec::new());
    let _ = vm.set_property(
        Value::Object(signal),
        "_listeners",
        Value::Object(listeners),
    );
    for method in ["addEventListener", "removeEventListener"] {
        let fn_ref = vm.alloc_native_fn(&format!("AbortSignal.{method}"));
        let _ = vm.set_property(Value::Object(signal), method, Value::Object(fn_ref));
    }
    let abort_fn = vm.alloc_native_fn("AbortSignal.abort");
    let _ = vm.set_property(Value::Object(signal), "abort", Value::Object(abort_fn));
    let tia_fn = vm.alloc_native_fn("AbortSignal.throwIfAborted");
    let _ = vm.set_property(
        Value::Object(signal),
        "throwIfAborted",
        Value::Object(tia_fn),
    );
    Ok(signal)
}

fn default_abort_error(vm: &mut Vm) -> Value {
    let err = vm.alloc_error_instance("This operation was aborted");
    let name = vm.alloc_string("AbortError".to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
    Value::Object(err)
}

pub(crate) fn apply_abort(vm: &mut Vm, signal: Value, reason: Value) -> Result<(), VmError> {
    if let Ok(Value::Boolean(true)) = vm.get_property(signal, "aborted") {
        return Ok(());
    }
    let _ = vm.set_property(signal, "aborted", Value::Boolean(true));
    let reason = if matches!(reason, Value::Undefined) {
        default_abort_error(vm)
    } else {
        reason
    };
    let _ = vm.set_property(signal, "reason", reason);
    if let Ok(Value::Object(arr)) = vm.get_property(signal, "_listeners") {
        let elements: Vec<Value> = match vm.heap.get(arr.0 as usize) {
            Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        };
        if !elements.is_empty() {
            let event = vm.alloc_ordinary();
            let type_str = vm.alloc_string("abort".to_owned());
            let _ = vm.set_property(Value::Object(event), "type", Value::Object(type_str));
            let event_val = Value::Object(event);
            for cb in elements {
                let _ = vm.invoke_callable(cb, Value::Undefined, &[event_val]);
            }
        }
    }
    Ok(())
}

pub(crate) fn abort_controller_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let controller = vm.alloc_ordinary();
    let signal = alloc_abort_signal(vm)?;
    let _ = vm.set_property(Value::Object(controller), "signal", Value::Object(signal));
    let abort_method = vm.alloc_native_fn("AbortController.abort");
    let _ = vm.set_property(
        Value::Object(controller),
        "abort",
        Value::Object(abort_method),
    );
    Ok(Value::Object(controller))
}

pub(crate) fn controller_abort_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let controller = crate::builtins::current_receiver();
    let signal = vm.get_property(controller, "signal")?;
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    apply_abort(vm, signal, reason)?;
    Ok(Value::Undefined)
}

pub(crate) fn abort_signal_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let signal = alloc_abort_signal(vm)?;
    Ok(Value::Object(signal))
}

pub(crate) fn abort_signal_abort_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    let is_signal = matches!(
        vm.get_property(this, "_isAbortSignal"),
        Ok(Value::Boolean(true))
    );
    if is_signal {
        return signal_abort_impl(vm, args);
    }
    let signal = alloc_abort_signal(vm)?;
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    apply_abort(vm, Value::Object(signal), reason)?;
    Ok(Value::Object(signal))
}

pub(crate) fn signal_throw_if_aborted(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    if let Ok(Value::Boolean(true)) = vm.get_property(this, "aborted") {
        let reason = match vm.get_property(this, "reason") {
            Ok(r) if !matches!(r, Value::Undefined) => r,
            _ => default_abort_error(vm),
        };
        return Err(VmError::Thrown(reason));
    }
    Ok(Value::Undefined)
}

fn signal_abort_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let signal = crate::builtins::current_receiver();
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    apply_abort(vm, signal, reason)?;
    Ok(Value::Undefined)
}

pub(crate) fn signal_add_event_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    let ty = args.first().copied().unwrap_or(Value::Undefined);
    if vm.format_value(ty) != "abort" {
        return Ok(Value::Undefined);
    }
    let Some(cb) = args.get(1).copied() else {
        return Ok(Value::Undefined);
    };
    let arr = match vm.get_property(this, "_listeners") {
        Ok(Value::Object(r)) => r,
        _ => vm.alloc_array(Vec::new()),
    };
    if let Some(crate::heap::HeapObject::Array { elements, .. }) = vm.heap.get_mut(arr.0 as usize) {
        elements.push(cb);
    }
    Ok(Value::Undefined)
}

pub(crate) fn signal_remove_event_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    let ty = args.first().copied().unwrap_or(Value::Undefined);
    if vm.format_value(ty) != "abort" {
        return Ok(Value::Undefined);
    }
    let Some(cb) = args.get(1).copied() else {
        return Ok(Value::Undefined);
    };
    if let Ok(Value::Object(arr)) = vm.get_property(this, "_listeners") {
        if let Some(crate::heap::HeapObject::Array { elements, .. }) =
            vm.heap.get_mut(arr.0 as usize)
        {
            if let Some(pos) = elements.iter().position(|e| e == &cb) {
                elements.remove(pos);
            }
        }
    }
    Ok(Value::Undefined)
}
