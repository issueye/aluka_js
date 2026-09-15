//! Error.captureStackTrace + 调用点（callsite）对象方法。

use crate::builtins::current_receiver;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

const CALLSITE_FRAMES: usize = 12;

/// `Error.prototype.toString()`：按规范 S20.5.3.4 组合 `name` 与 `message`。
///
/// - `name` 缺省 → `"Error"`；`message` 缺省 / 空串 → 只输出 name；
/// - 两者皆为空串 → 返回空串（规范允许，V8 亦如此）。
///
/// 供 `String(err)`、模板串插值、未捕获异常渲染统一复用（此前依赖各调用点
/// 自行读 `name`/`message` 拼接，形态不一）。
pub(crate) fn error_proto_to_string(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let name = vm
        .get_property(receiver, "name")
        .ok()
        .map(|v| vm.format_value(v))
        .filter(|s| s != "undefined")
        .unwrap_or_else(|| "Error".to_owned());
    let message = vm
        .get_property(receiver, "message")
        .ok()
        .map(|v| vm.format_value(v))
        .filter(|s| s != "undefined")
        .unwrap_or_default();
    let text = match (name.is_empty(), message.is_empty()) {
        (true, true) => String::new(),
        (true, false) => message,
        (false, true) => name,
        (false, false) => format!("{name}: {message}"),
    };
    Ok(Value::Object(vm.alloc_string(text)))
}

pub(crate) fn error_capture_stack_trace(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(target) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
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
    let method = match receiver.case() {
        ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { name, .. }) => {
                name.clone().split('.').next_back().unwrap_or("").to_owned()
            }
            _ => String::new(),
        },
        _ => String::new(),
    };
    let file = match receiver.case() {
        ValueCase::Object(r) => vm
            .own_value(r.0 as usize, "_file")
            .map(|v| vm.format_value(v))
            .unwrap_or_default(),
        _ => String::new(),
    };
    match method.as_str() {
        "getFileName" => Ok(Value::Object(vm.alloc_string(file))),
        "getLineNumber" | "getColumnNumber" => Ok(Value::Number(0.0)),
        "isNative" | "isEval" | "isConstructor" => Ok(Value::Boolean(false)),
        "getFunctionName" | "getTypeName" => Ok(Value::Undefined),
        "toString" => Ok(Value::Object(
            vm.alloc_string(format!("at <anonymous> ({file})")),
        )),
        _ => Ok(Value::Undefined),
    }
}
