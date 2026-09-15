//! Error.captureStackTrace + 调用点（callsite）对象方法。

use crate::builtins::current_receiver;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

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

/// `Error.captureStackTrace(targetObject[, constructorOpt])`：把 `targetObject.stack`
/// 重写为按当前调用链生成的**字符串**（Node 形态）。
///
/// 语义（Node 实测锁定）：
/// - 返回 `undefined`；
/// - `stack` **保持字符串**（此前写「调用点数组」，与 `stack` 的字符串形态
///   冲突，且 `String(err.stack)` 会得到 `[object Object]` 形态）；
/// - 第二参数 `constructorOpt`：**省略该构造器帧及其内侧帧**（规范
///   「all frames above constructorOpt, including constructorOpt, will be omitted」）——
///   子类里 `Error.captureStackTrace(this, MyErr)` 的 stack 不应出现 `MyErr`；
/// - 帧数受 `Error.stackTraceLimit` 限制（同普通 `stack` 生成）。
pub(crate) fn error_capture_stack_trace(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(target) = args.first().copied().and_then(|v| v.as_object()) else {
        return Ok(Value::Undefined);
    };
    let constructor_opt = args.get(1).copied().and_then(|v| v.as_object());
    vm.fill_error_stack(target, constructor_opt);
    Ok(Value::Undefined)
}

/// callsite 对象的方法面（`Error.prepareStackTrace` 的实参元素）。
///
/// 真实包（`depd/index.js::callSiteLocation`）会调用
/// `getFileName`/`getLineNumber`/`getColumnNumber`/`isEval`/`getEvalOrigin`/
/// `getFunctionName`，并读 `getThis`/`getTypeName` 生成消息。
/// 本运行时无源映射：行号 0、列号 1、`isEval`/`isNative`/`isConstructor` 均 false。
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
    let field = |vm: &mut Vm, key: &str| match receiver.case() {
        ValueCase::Object(r) => vm
            .own_value(r.0 as usize, key)
            .map(|v| vm.format_value(v))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let file = field(vm, "_file");
    let func_name = field(vm, "_funcName");
    match method.as_str() {
        "getFileName" => Ok(Value::Object(vm.alloc_string(file))),
        "getLineNumber" => Ok(Value::Number(0.0)),
        "getColumnNumber" => Ok(Value::Number(1.0)),
        "getFunctionName" => {
            if func_name.is_empty() {
                Ok(Value::Undefined)
            } else {
                Ok(Value::Object(vm.alloc_string(func_name)))
            }
        }
        // `getTypeName`：无接收者信息 → undefined（`depd` 对其做真值判断）
        "getTypeName" | "getEvalOrigin" => Ok(Value::Undefined),
        // `getThis`：未跟踪接收者 → undefined（`depd` 会 `context && …` 短路）
        "getThis" => Ok(Value::Undefined),
        "isNative" | "isEval" | "isConstructor" => Ok(Value::Boolean(false)),
        "isToplevel" => Ok(Value::Boolean(true)),
        "toString" => Ok(Value::Object(vm.alloc_string(if func_name.is_empty() {
            format!("at <anonymous> ({file})")
        } else {
            format!("at {func_name} ({file})")
        }))),
        _ => Ok(Value::Undefined),
    }
}
