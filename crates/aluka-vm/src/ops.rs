//! 算术、位运算与类型强制转换逻辑。

use crate::heap::HeapObject;
use crate::interpreter::Vm;
use crate::value::Value;

/// 将任意值强制转换为数值。
#[must_use]
pub fn to_number(val: Value) -> f64 {
    match val {
        Value::Number(n) => n,
        Value::Boolean(true) => 1.0,
        Value::Boolean(false) | Value::Null => 0.0,
        Value::Undefined => f64::NAN,
        Value::Object(_) => f64::NAN,
    }
}

/// 将任意值强制转换为布尔值（ECMAScript ToBoolean）。
///
/// 字符串是堆对象，空字符串必须为 falsy——需要堆访问：
/// `Object` 引用先查堆，`HeapObject::String` 按内容非空判定，
/// 其余对象一律 truthy。`heap` 为 `&[]`（JIT 无堆路径）时
/// 字符串按 truthy 处理（与旧行为一致，JIT 通道另行完善）。
#[must_use]
pub fn to_boolean(val: Value, heap: &[HeapObject]) -> bool {
    match val {
        Value::Undefined | Value::Null => false,
        Value::Boolean(b) => b,
        Value::Number(n) => n != 0.0 && !n.is_nan(),
        Value::Object(r) => match heap.get(r.0 as usize) {
            Some(HeapObject::String(s)) => !s.is_empty(),
            _ => true,
        },
    }
}

/// 字符串值相等：两个堆字符串按内容比较（JS 语义；句柄相同或内容相同）。
pub fn string_values_eq(a: &Value, b: &Value, heap: &[HeapObject]) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            if x == y {
                return true;
            }
            match (heap.get(x.0 as usize), heap.get(y.0 as usize)) {
                (Some(HeapObject::String(sa)), Some(HeapObject::String(sb))) => sa == sb,
                _ => false,
            }
        }
        _ => false,
    }
}

fn get_string_repr<'a>(
    idx: usize,
    heap: &'a [HeapObject],
    constants: &'a [aluka_bytecode::Constant],
) -> Option<&'a str> {
    if idx < heap.len() {
        if let HeapObject::String(s) = &heap[idx] {
            return Some(s.as_str());
        }
    }
    if let Some(aluka_bytecode::Constant::String(s)) = constants.get(idx) {
        return Some(s.as_str());
    }
    None
}

/// 判定非严格相等（==）。
pub fn eq(
    left: Value,
    right: Value,
    heap: &[HeapObject],
    constants: &[aluka_bytecode::Constant],
) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Null, Value::Null) | (Value::Undefined, Value::Undefined) => true,
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
        (Value::Number(n), Value::Object(r)) | (Value::Object(r), Value::Number(n)) => {
            if let Some(s) = get_string_repr(r.0 as usize, heap, constants) {
                if let Ok(sn) = s.trim().parse::<f64>() {
                    return n == sn;
                }
            }
            false
        }
        (Value::Object(a), Value::Object(b)) => {
            if a == b {
                true
            } else {
                let s_a = get_string_repr(a.0 as usize, heap, constants);
                let s_b = get_string_repr(b.0 as usize, heap, constants);
                match (s_a, s_b) {
                    (Some(sa), Some(sb)) => sa == sb,
                    _ => false,
                }
            }
        }
        _ => false,
    }
}

/// 判定严格相等（===）。
pub fn strict_eq(
    left: Value,
    right: Value,
    heap: &[HeapObject],
    constants: &[aluka_bytecode::Constant],
) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Null, Value::Null) | (Value::Undefined, Value::Undefined) => true,
        (Value::Object(a), Value::Object(b)) => {
            if a == b {
                true
            } else {
                let s_a = get_string_repr(a.0 as usize, heap, constants);
                let s_b = get_string_repr(b.0 as usize, heap, constants);
                match (s_a, s_b) {
                    (Some(sa), Some(sb)) => sa == sb,
                    _ => false,
                }
            }
        }
        _ => false,
    }
}

impl Vm {
    /// ECMAScript ToBoolean（借助本 VM 堆判定字符串内容）。
    #[must_use]
    pub fn truthy(&self, val: Value) -> bool {
        to_boolean(val, &self.heap)
    }

    /// 执行加法运算（支持数值相加与 ECMAScript 字符串自动拼接）。
    pub fn add_values(&mut self, left: Value, right: Value) -> Value {
        if let (Value::Number(a), Value::Number(b)) = (left, right) {
            return Value::Number(a + b);
        }
        let is_left_str = if let Value::Object(r) = left {
            matches!(self.heap.get(r.0 as usize), Some(HeapObject::String(_)))
        } else {
            false
        };
        let is_right_str = if let Value::Object(r) = right {
            matches!(self.heap.get(r.0 as usize), Some(HeapObject::String(_)))
        } else {
            false
        };
        // Buffer 参与 `+`：ToPrimitive 走 toString（utf8 内容），Node 语义
        // `'' + Buffer.from('ok')` → "ok"（而非 "[object Object]"）。
        let is_left_buf = self.is_buffer_value(left);
        let is_right_buf = self.is_buffer_value(right);

        if is_left_str || is_right_str || is_left_buf || is_right_buf {
            let s1 = self.value_as_concat_text(left);
            let s2 = self.value_as_concat_text(right);
            let combined = format!("{s1}{s2}");
            let s_ref = self.alloc_string(combined);
            return Value::Object(s_ref);
        }

        Value::Number(f64::NAN)
    }

    /// 值是否为 Buffer 实例（`_isBuffer` 标记）。
    fn is_buffer_value(&self, v: Value) -> bool {
        matches!(v, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isBuffer"))
    }

    /// 字符串拼接语境下的文本化：Buffer → utf8 内容，其余 format_value。
    fn value_as_concat_text(&mut self, v: Value) -> String {
        if self.is_buffer_value(v) {
            let bytes = crate::builtins::buffer::extract_bytes(self, v).unwrap_or_default();
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        self.format_value(v)
    }
}
