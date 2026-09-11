//! node:test 断言实现（Phase 8）：结构深度比较与错误消息格式化。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodetest/test_assert.go`）：
//! `deepStrictEqual`/`deepEqual` 递归结构比较、正则匹配与
//! `testErrorMessage`（从异常值提取 JS 错误消息）。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

/// 原始值/引用严格相等（对齐 Go `nodebase.StrictEqual`：NaN 不等、
/// 字符串比内容、对象比引用）。
pub fn strict_equal(vm: &Vm, a: Value, b: Value) -> bool {
    match (a, b).case() {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (ValueCase::Boolean(x), ValueCase::Boolean(y)) => x == y,
        (ValueCase::Number(x), ValueCase::Number(y)) => x == y,
        (ValueCase::Object(x), ValueCase::Object(y)) => {
            match (vm.heap.get(x.index()), vm.heap.get(y.index())) {
                (Some(HeapObject::String(sa)), Some(HeapObject::String(sb))) => sa == sb,
                (Some(HeapObject::BigInt(sa)), Some(HeapObject::BigInt(sb))) => sa == sb,
                _ => x == y,
            }
        }
        _ => false,
    }
}

/// 宽松相等（对齐 Go `nodebase.LooseEqual` 探测域：严格相等，
/// 或数字与数字样字符串的转换比较）。
pub fn loose_equal(vm: &Vm, a: Value, b: Value) -> bool {
    if strict_equal(vm, a, b) {
        return true;
    }
    if let (ValueCase::Number(x), ValueCase::Object(y)) = (a, b) {
        if let Some(HeapObject::String(s)) = vm.heap.get(y.index()) {
            if let Ok(n) = s.trim().parse::<f64>() {
                return x == n;
            }
        }
    }
    if let (ValueCase::Object(x), ValueCase::Number(y)) = (a, b) {
        if let Some(HeapObject::String(s)) = vm.heap.get(x.index()) {
            if let Ok(n) = s.trim().parse::<f64>() {
                return n == y;
            }
        }
    }
    false
}

/// 是否数组值。
fn is_array(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Array { .. })))
}

/// 是否普通对象。
fn is_ordinary(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })))
}

/// 递归严格深度相等（Node `assert.deepStrictEqual` 语义）：数组逐元素；
/// 对象键集一致且每键值严格深等；其余走严格相等。
pub fn deep_strict_equal(vm: &mut Vm, a: Value, b: Value) -> bool {
    if strict_equal(vm, a, b) {
        return true;
    }
    if is_array(vm, a) || is_array(vm, b) {
        if !is_array(vm, a) || !is_array(vm, b) {
            return false;
        }
        let (Some(ValueCase::Object(ra)), Some(ValueCase::Object(rb))) = (Some(a), Some(b)) else {
            unreachable!()
        };
        let (elems_a, elems_b) = match (vm.heap.get(ra.index()), vm.heap.get(rb.index())) {
            (
                Some(HeapObject::Array { elements: ea, .. }),
                Some(HeapObject::Array { elements: eb, .. }),
            ) => (ea.clone(), eb.clone()),
            _ => unreachable!("已确认数组"),
        };
        if elems_a.len() != elems_b.len() {
            return false;
        }
        return elems_a
            .iter()
            .zip(elems_b.iter())
            .all(|(x, y)| deep_strict_equal(vm, *x, *y));
    }
    if is_ordinary(vm, a) && is_ordinary(vm, b) {
        let (Some(ValueCase::Object(ra)), Some(ValueCase::Object(rb))) = (Some(a), Some(b)) else {
            unreachable!()
        };
        let props_a = vm.own_entries(ra.index());
        let props_b: std::collections::HashMap<String, Value> =
            vm.own_entries(rb.index()).into_iter().collect();
        if props_a.len() != props_b.len() {
            return false;
        }
        for (k, va) in &props_a {
            match props_b.get(k) {
                Some(vb) => {
                    if !deep_strict_equal(vm, *va, *vb) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        return true;
    }
    // 剩余情形：类型不同不等；同类型比字符串化结果（对齐 Go 兜底）。
    vm.format_value(a) == vm.format_value(b)
}

/// 递归宽松深度相等（`==` 语义：数字/字符串可转换比较）。
pub fn deep_loose_equal(vm: &mut Vm, a: Value, b: Value) -> bool {
    if deep_strict_equal(vm, a, b) {
        return true;
    }
    if let (ValueCase::Number(x), ValueCase::Object(y)) = (a, b) {
        if let Some(HeapObject::String(s)) = vm.heap.get(y.index()) {
            if let Ok(n) = s.trim().parse::<f64>() {
                return x == n;
            }
        }
    }
    if let (ValueCase::Object(x), ValueCase::Number(y)) = (a, b) {
        if let Some(HeapObject::String(s)) = vm.heap.get(x.index()) {
            if let Ok(n) = s.trim().parse::<f64>() {
                return n == y;
            }
        }
    }
    false
}

/// 正则整体匹配（对齐 Go `vmRegexpTest`：`re.test(target)` 语义）。
pub fn regexp_test(vm: &mut Vm, re: Value, target: &str) -> bool {
    let ValueCase::Object(r) = re.case() else {
        return false;
    };
    let (pattern, flags) = match vm.heap.get(r.index()) {
        Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
        _ => return false,
    };
    let Ok(compiled) = aluka_regex::Regex::compile(&pattern, &flags) else {
        return false;
    };
    compiled.test(target).unwrap_or(false)
}

/// 从异常错误提取 JS 错误消息（对齐 Go `testErrorMessage`）：
/// 抛出对象带 message 属性用之；抛出原始值字符串化；其余用错误文本。
pub fn error_message(vm: &mut Vm, err: &VmError) -> String {
    let VmError::Thrown(v) = err else {
        return err.to_string();
    };
    if let Some(r) = v.as_object() {
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })) {
            if let Ok(msg) = vm.get_property(*v, "message") {
                if !matches!(msg, Value::Undefined | Value::Null) {
                    return vm.format_value(msg);
                }
            }
        }
    }
    vm.format_value(*v)
}

/// t.assert 系列失败消息（对齐 Go `engine.ErrAssertion` 前缀）。
pub fn assert_fail(vm: &mut Vm, detail: &str) -> VmError {
    thrown_msg(vm, &format!("aluka: assertion error: {detail}"))
}

/// t.assert 类型错误消息（对齐 Go `engine.ErrTypeError` 前缀）。
pub fn type_fail(vm: &mut Vm, detail: &str) -> VmError {
    thrown_msg(vm, &format!("aluka: type error: {detail}"))
}

/// 构造带 message 的错误异常值（Go 侧为 JS Error 对象语义）。
pub fn thrown_msg(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}
