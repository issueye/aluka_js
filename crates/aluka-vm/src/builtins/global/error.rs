//! Error.captureStackTrace + 调用点（callsite）对象方法。

use crate::builtins::current_receiver;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

const CALLSITE_FRAMES: usize = 12;

pub(crate) fn error_capture_stack_trace(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(target) = args.first().copied() else { return Ok(Value::Undefined); };
    let mut frames = Vec::with_capacity(CALLSITE_FRAMES);
    for _ in 0..CALLSITE_FRAMES {
        let site = vm.alloc_ordinary();
        let ns = vm.alloc_string("callsite".to_owned());
        let _ = vm.set_property(Value::Object(site), "_builtinNs", Value::Object(ns));
        let file = vm.alloc_string(vm.entry_file.clone());
        let _ = vm.set_property(Value::Object(site), "_file", Value::Object(file));
        frames.push(Value::Object(site));
    }
    let stack_arr = Value::Object(vm.alloc_array(frames));
    vm.set_property(target, "stack", stack_arr)?;
    Ok(Value::Undefined)
}

pub(crate) fn callsite_method(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let method = match receiver {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { name, .. }) => name.clone().split('.').next_back().unwrap_or("").to_owned(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let file = match receiver {
        Value::Object(r) => vm.own_value(r.0 as usize, "_file").map(|v| vm.format_value(v)).unwrap_or_default(),
        _ => String::new(),
    };
    match method.as_str() {
        "getFileName" => Ok(Value::Object(vm.alloc_string(file))),
        "getLineNumber" | "getColumnNumber" => Ok(Value::Number(0.0)),
        "isNative" | "isEval" | "isConstructor" => Ok(Value::Boolean(false)),
        "getFunctionName" | "getTypeName" => Ok(Value::Undefined),
        "toString" => Ok(Value::Object(vm.alloc_string(format!("at <anonymous> ({file})")))),
        _ => Ok(Value::Undefined),
    }
}