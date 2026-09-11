//! EventTarget / Event / CustomEvent 全局构造器。

use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

pub(crate) fn is_callable(vm: &Vm, v: Value) -> bool {
    crate::builtins::readline::is_callable_value(vm, v)
}

pub(crate) fn event_target_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let target = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(target),
        "_isEventTarget",
        Value::Boolean(true),
    );
    let ns = vm.alloc_string("EventTarget".to_owned());
    let _ = vm.set_property(Value::Object(target), "_builtinNs", Value::Object(ns));
    let map = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(target), "_etListeners", Value::Object(map));
    for method in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        let f = vm.alloc_native_fn(&format!("EventTarget.{method}"));
        let _ = vm.set_property(Value::Object(target), method, Value::Object(f));
    }
    Ok(Value::Object(target))
}

pub(crate) fn event_target_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = crate::builtins::pending_native_name();
    let target = crate::builtins::current_receiver();
    let first = args.first().copied().unwrap_or(Value::Undefined);
    let ev = if let Ok(ValueCase::Object(_)) = vm.get_property(first, "type").map(ValueCase::from) {
        vm.get_property(first, "type")
            .map(|v| vm.format_value(v))
            .unwrap_or_default()
    } else {
        vm.format_value(first)
    };
    let cb = args.get(1).copied();
    let map = match vm.get_property(target, "_etListeners").map(|v| v.case()) {
        Ok(ValueCase::Object(m)) => m,
        _ => return Ok(Value::Undefined),
    };
    let arr = match vm.get_property(Value::Object(map), &ev).map(|v| v.case()) {
        Ok(ValueCase::Object(a)) => a,
        _ => {
            if name.ends_with("addEventListener") {
                let new_arr = vm.alloc_array(Vec::new());
                let _ = vm.set_property(Value::Object(map), &ev, Value::Object(new_arr));
                new_arr
            } else {
                return Ok(match name.as_str() {
                    "EventTarget.dispatchEvent" => Value::Boolean(false),
                    _ => Value::Undefined,
                });
            }
        }
    };
    match name.as_str() {
        "EventTarget.addEventListener" => {
            if let Some(cb) = cb.filter(|v| is_callable(vm, *v)) {
                if let Some(crate::heap::HeapObject::Array { elements, .. }) =
                    vm.heap.get_mut(arr.0 as usize)
                {
                    if !elements.contains(&cb) {
                        elements.push(cb);
                    }
                }
            }
            Ok(Value::Undefined)
        }
        "EventTarget.removeEventListener" => {
            if let Some(cb) = cb {
                if let Some(crate::heap::HeapObject::Array { elements, .. }) =
                    vm.heap.get_mut(arr.0 as usize)
                {
                    elements.retain(
                        |e| !matches!((e.case(), cb.case()), (ValueCase::Object(a), ValueCase::Object(b)) if a == b),
                    );
                }
            }
            Ok(Value::Undefined)
        }
        _ => {
            let callbacks: Vec<Value> = match vm.heap.get(arr.0 as usize) {
                Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
                _ => Vec::new(),
            };
            let has = !callbacks.is_empty();
            if let Some(e) = args
                .first()
                .copied()
                .unwrap_or(Value::Undefined)
                .as_object()
            {
                let _ = vm.set_property(Value::Object(e), "target", target);
            }
            let event_val = args.first().copied().unwrap_or(Value::Undefined);
            for cb in callbacks {
                let _ = vm.invoke_callable(cb, Value::Undefined, &[event_val]);
            }
            Ok(Value::Boolean(has))
        }
    }
}

pub(crate) fn event_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ty = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let event = vm.alloc_ordinary();
    let ty_val = vm.alloc_string(ty);
    let _ = vm.set_property(Value::Object(event), "type", Value::Object(ty_val));
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);
    let bubbles = vm
        .get_property(opts, "bubbles")
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let cancelable = vm
        .get_property(opts, "cancelable")
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let _ = vm.set_property(Value::Object(event), "bubbles", Value::Boolean(bubbles));
    let _ = vm.set_property(
        Value::Object(event),
        "cancelable",
        Value::Boolean(cancelable),
    );
    let _ = vm.set_property(
        Value::Object(event),
        "defaultPrevented",
        Value::Boolean(false),
    );
    let _ = vm.set_property(Value::Object(event), "_isEvent", Value::Boolean(true));
    let pdf = vm.alloc_native_fn("Event.preventDefault");
    let _ = vm.set_property(Value::Object(event), "preventDefault", Value::Object(pdf));
    Ok(Value::Object(event))
}

pub(crate) fn event_prevent_default(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    if let Ok(ValueCase::Boolean(true)) = vm.get_property(this, "cancelable").map(ValueCase::from) {
        let _ = vm.set_property(this, "defaultPrevented", Value::Boolean(true));
    }
    Ok(Value::Undefined)
}

pub(crate) fn custom_event_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let event = event_ctor_impl(vm, args)?;
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);
    let detail = match vm.get_property(opts, "detail") {
        Ok(d) if !matches!(d, Value::Undefined) => d,
        _ => Value::Null,
    };
    let _ = vm.set_property(event, "detail", detail);
    let _ = vm.set_property(event, "_isCustomEvent", Value::Boolean(true));
    Ok(event)
}
