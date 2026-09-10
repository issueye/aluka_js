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

/// ECMAScript `ToString(Number)`：最短有效数字 + 指数切换规则
/// （k ≤ n ≤ 21 补零；0 < n ≤ 21 插小数点；-6 < n ≤ 0 前导 0.；
/// 其余科学计数法 `d.ddde±x`）。Rust `{}` 不产指数形态、`{:e}` 恒为指数形态——
/// 借 [`crate::bigdec::shortest_spec_digits`] 取规范要求的有效数字后按上述规则重排。
#[must_use]
pub fn js_number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_owned();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        };
    }
    let a = n.abs();
    if a == 0.0 {
        // 规范：String(-0) === "0"（-0 的负号在字符串化时丢弃）
        return "0".to_owned();
    }
    let neg = n.is_sign_negative();
    // 最短有效数字 + 规范要求的「并列取偶」（Rust `{:e}` 并列时取较大者，
    // 需借精确展开修正；详见 `bigdec::shortest_spec_digits`）
    let (digits, n) = crate::bigdec::shortest_spec_digits(a);
    let digits = digits.as_str();
    let k = digits.len() as i32;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let e = n - 1;
        if e >= 0 {
            out.push('+');
            out.push_str(&e.to_string());
        } else {
            out.push_str(&e.to_string());
        }
    }
    out
}

/// JS 数字字符串 → f64（ECMAScript `StringToNumber`）：
/// 空白裁剪后空串为 0；`0x/0o/0b` 进制前缀按对应进制；其余走 Rust
/// f64 解析（`Infinity`/`inf`/`NaN` 等 Rust 已按大小写不敏感支持）。
#[must_use]
pub fn parse_js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(oct) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return i64::from_str_radix(oct, 8)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return i64::from_str_radix(bin, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
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
        // 双方都不是数值：任一为对象 → ToPrimitive 后字符串拼接
        // （`[] + []` === ""、`[] + {}` === "[object Object]"；生成语料实测）
        let left_obj = matches!(left, Value::Object(_));
        let right_obj = matches!(right, Value::Object(_));
        if left_obj || right_obj {
            let s1 = self.value_as_concat_text(left);
            let s2 = self.value_as_concat_text(right);
            let s_ref = self.alloc_string(format!("{s1}{s2}"));
            return Value::Object(s_ref);
        }

        // 双方皆原始值且非字符串 → **数值相加**（规范 `+` 的最后一步：
        // ToNumeric 后相加）。此前直接落到 `NaN`，导致
        // `true + 1` → NaN（Node 2）、`true + true` → NaN（Node 2）、
        // `null + 1` → NaN（Node 1）——布尔/null 参与算术的常见写法全错。
        // `undefined + 1` 仍为 NaN（规范如此）。
        Value::Number(self.to_number_value(left) + self.to_number_value(right))
    }

    /// 值是否为 Buffer 实例（`_isBuffer` 标记）。
    fn is_buffer_value(&self, v: Value) -> bool {
        matches!(v, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isBuffer"))
    }

    /// ECMAScript 抽象关系比较（`<`）的求值核心。
    ///
    /// 返回 `Some(true/false)` 为确定结果；`None` 表示任一操作数经 ToNumber 得
    /// `NaN`（调用方按规范对 `<` / `<=` / `>` / `>=` 分别处理）。
    ///
    /// 规范要点（此前 `Op::Lt/Le/Gt/Ge` 直接做纯数值比较，**字符串比较恒 false**）：
    /// - 两侧皆为字符串 → 按 **UTF-16 码元**字典序比较（`"a" < "b"`、`"A" < "a"`、
    ///   `"2" > "10"` 因此为 true；注意与码点序在非 BMP 字符上有别）；
    /// - 否则两侧 ToNumber 后数值比较（`1 < "2"` → `1 < 2` → true）。
    pub(crate) fn js_less_than(&self, l: Value, r: Value) -> Option<bool> {
        // 规范第一步：两侧 ToPrimitive（hint number）——对象由此得到原始值
        // （数组 → join 字符串、Date → 时间值、普通对象 → "[object Object]"）。
        let lp = self.to_cmp_primitive(l);
        let rp = self.to_cmp_primitive(r);
        // 两个原始值**皆为字符串** → UTF-16 码元序比较（`['a'] < ['b']` 亦走此路，
        // 因为数组的 ToPrimitive 结果是字符串）
        if let (CmpPrimitive::Str(a), CmpPrimitive::Str(b)) = (&lp, &rp) {
            return Some(utf16_cmp(a, b) == std::cmp::Ordering::Less);
        }
        // 否则走数值比较（`1 < "2"` → 1 < 2）
        let (x, y) = (lp.to_number(), rp.to_number());
        if x.is_nan() || y.is_nan() {
            return None;
        }
        Some(x < y)
    }

    /// 关系比较用的 **ToPrimitive(hint number)**：对象按其可原始化结果返回
    /// 数值或字符串（决定关系比较走字符串序还是数值序）。
    fn to_cmp_primitive(&self, v: Value) -> CmpPrimitive {
        match v {
            Value::Number(n) => CmpPrimitive::Num(n),
            Value::Boolean(_) | Value::Null | Value::Undefined => CmpPrimitive::Num(to_number(v)),
            Value::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(s)) => CmpPrimitive::Str(s.clone()),
                Some(HeapObject::BigInt(b)) => {
                    CmpPrimitive::Num(b.trim().parse::<f64>().unwrap_or(f64::NAN))
                }
                _ => match self.own_value(r.0 as usize, "_timeValue") {
                    // Date 实例：valueOf 产原始数值 → 按时间值比较
                    Some(Value::Number(t)) => CmpPrimitive::Num(t),
                    // 其余对象：valueOf 不产原始值 → toString（数组 join /
                    // 普通对象 "[object Object]"）
                    _ => CmpPrimitive::Str(self.format_value(v)),
                },
            },
        }
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

/// 关系比较用的 ToPrimitive 结果（决定走字符串序还是数值序）。
pub(crate) enum CmpPrimitive {
    /// 原始数值（Number / Boolean / Null / Undefined / BigInt / Date）
    Num(f64),
    /// 原始字符串（String 与「toString 产字符串」的对象）
    Str(String),
}

impl CmpPrimitive {
    /// 数值比较用：字符串按 `ToNumber` 解析（`"2"` → 2、非数字串 → NaN）。
    fn to_number(&self) -> f64 {
        match self {
            CmpPrimitive::Num(n) => *n,
            CmpPrimitive::Str(s) => parse_js_number(s),
        }
    }
}

/// 按 **UTF-16 码元序**比较两个字符串（ECMAScript 关系比较语义）。
///
/// Rust 的 `str` 比较基于 UTF-8 字节序，等价于**码点序**；与规范要求的 UTF-16
/// 码元序仅在「非 BMP 字符（U+10000+，UTF-16 为代理对）与 U+E000..U+FFFF 字符」
/// 的相对次序上不同，故此处显式按码元比较（`js_less_than` 使用）。
pub(crate) fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.encode_utf16();
    let mut bi = b.encode_utf16();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
        }
    }
}
