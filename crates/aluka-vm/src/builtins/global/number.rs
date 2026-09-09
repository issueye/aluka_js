//! Number 构造器静态方法。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;

pub(crate) fn number_static(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 方法名取自被调函数（`Number.isInteger`），而非 receiver——普通调用
    // `Number.isInteger(3)` 的 receiver 是 Number 构造器对象（NativeCtor），
    // 按.receiver 推导会得到空名落进 `_ => undefined`（is-odd 生态实测暴露）。
    let name = crate::builtins::pending_native_name();
    let method = name.split('.').next_back().unwrap_or("");
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let to_num = |vm: &mut Vm, v: Value| -> f64 { vm.to_number_value(v) };
    match method {
        "isInteger" => {
            let n = to_num(vm, v);
            Ok(Value::Boolean(
                matches!(v, Value::Number(_)) && n.fract() == 0.0 && n.is_finite(),
            ))
        }
        "isSafeInteger" => {
            let n = to_num(vm, v);
            Ok(Value::Boolean(
                matches!(v, Value::Number(_))
                    && n.fract() == 0.0
                    && n.is_finite()
                    && n.abs() <= 9007199254740991.0,
            ))
        }
        "isFinite" => {
            let n = to_num(vm, v);
            Ok(Value::Boolean(
                matches!(v, Value::Number(_)) && n.is_finite(),
            ))
        }
        "isNaN" => Ok(Value::Boolean(to_num(vm, v).is_nan())),
        "parseInt" => super::core_fn::global_parse_int(vm, args),
        "parseFloat" => super::core_fn::global_parse_float(vm, args),
        _ => Ok(Value::Undefined),
    }
}
