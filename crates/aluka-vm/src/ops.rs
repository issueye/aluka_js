//! 算术、位运算与类型强制转换逻辑。

use crate::VmError;
use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::Vm;
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// 将任意值强制转换为数值。
#[must_use]
pub fn to_number(val: Value) -> f64 {
    match val.case() {
        ValueCase::Number(n) => n,
        ValueCase::Boolean(true) => 1.0,
        ValueCase::Boolean(false) | ValueCase::Null => 0.0,
        ValueCase::Undefined => f64::NAN,
        ValueCase::Object(_) => f64::NAN,
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
    // 规范只认精确的 "Infinity"（含前导 +/-）；Rust 的 f64::parse 还接受
    // "INFINITY"/"inf"/"infinity"/"NaN" 等——先拦下这些宽松形态，避免
    // `Number("INFINITY")` 误得 Infinity（应为 NaN，S15.7.1.1 族）
    let (sign_ok, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (true, t.strip_prefix('+').unwrap_or(t)),
    };
    let _ = sign_ok;
    let lower = body.to_ascii_lowercase();
    if lower.starts_with("inf") || lower.starts_with("nan") {
        return if t == "Infinity" || t == "+Infinity" {
            f64::INFINITY
        } else if t == "-Infinity" {
            f64::NEG_INFINITY
        } else {
            f64::NAN
        };
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
/// 规范 `ToInt32`：先 ToNumber，再按 2^32 取模回绕到有符号 32 位。
///
/// Rust 的 `f64 as i32` 是**饱和转换**（NaN→0、±Inf → 极值、超范围钳制），
/// 与规范的模回绕不同——`(2147483647+1)|0` 应得 -2147483648 而非 2147483647。
/// 字符串的 **UTF-16 码元**长度（规范 String.length 口径）：非 BMP 码点占 2。
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| if c > '\u{FFFF}' { 2 } else { 1 }).sum()
}

/// 规范 `ToInt32`：先 ToNumber 再按 2^32 取模回绕到有符号 32 位。
///
/// Rust 的 `f64 as i32` 是**饱和转换**（NaN→0、超范围钳制），与规范的模
/// 回绕不同——`(2147483647+1)|0` 应得 -2147483648 而非 2147483647。
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    // 取模回绕：先归约到 [0, 2^32) 再解释为有符号
    let m = n.trunc() % 4294967296.0;
    let u = if m < 0.0 { m + 4294967296.0 } else { m };
    u as u32 as i32
}

/// 规范 `ToUint32`：同 ToInt32 但按无符号解释。
pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

/// 规范 `ToBoolean`（BigInt 为原始值语义：0n 假、其余真）。
pub fn to_boolean(val: Value, heap: &[HeapObject]) -> bool {
    match val.case() {
        ValueCase::Undefined | ValueCase::Null => false,
        ValueCase::Boolean(b) => b,
        ValueCase::Number(n) => n != 0.0 && !n.is_nan(),
        ValueCase::Object(r) => match heap.get(r.0 as usize) {
            Some(HeapObject::String(s)) => !s.is_empty(),
            // BigInt 是原始值语义：**0n 为假**，其余为真
            //（`Boolean(0n) === false`；此前落 `_ => true` 致 0n 恒真）
            Some(HeapObject::BigInt(text)) => text.trim_start_matches(['-', '+']) != "0",
            _ => true,
        },
    }
}

/// 字符串值相等：两个堆字符串按内容比较（JS 语义；句柄相同或内容相同）。
pub fn string_values_eq(a: &Value, b: &Value, heap: &[HeapObject]) -> bool {
    match (a.case(), b.case()) {
        (ValueCase::Object(x), ValueCase::Object(y)) => {
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

/// 包装实例数据槽读取（eq 纯堆面）：Ordinary + Dict 模式下的
/// `[[NumberValue]]`/`[[BooleanValue]]`/`[[StringValue]]`。
fn wrapper_data(heap: &[HeapObject], r: u32) -> Option<&Value> {
    match heap.get(r as usize) {
        Some(HeapObject::Ordinary {
            props: OrdinaryProps::Dict { properties, .. },
            ..
        }) => properties
            .iter()
            .find(|(k, _)| {
                matches!(
                    k.as_str(),
                    "[[NumberValue]]" | "[[BooleanValue]]" | "[[BooleanData]]" | "[[StringValue]]"
                )
            })
            .map(|(_, v)| v),
        _ => None,
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

/// BigInt 堆对象 → 归一化十进制文本（`===` 按数值内容比较，非对象句柄）。
fn get_bigint_repr(idx: usize, heap: &[HeapObject]) -> Option<&str> {
    match heap.get(idx) {
        Some(HeapObject::BigInt(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// 判定非严格相等（==）。
pub fn eq(
    left: Value,
    right: Value,
    heap: &[HeapObject],
    constants: &[aluka_bytecode::Constant],
) -> bool {
    match (left.case(), right.case()) {
        (ValueCase::Number(a), ValueCase::Number(b)) => a == b,
        (ValueCase::Boolean(a), ValueCase::Boolean(b)) => a == b,
        (ValueCase::Null, ValueCase::Null) | (ValueCase::Undefined, ValueCase::Undefined) => true,
        (ValueCase::Null, ValueCase::Undefined) | (ValueCase::Undefined, ValueCase::Null) => true,
        (ValueCase::Number(n), ValueCase::Object(r))
        | (ValueCase::Object(r), ValueCase::Number(n)) => {
            // BigInt ↔ Number：按**数学值**比较（`1n == 1` 为 true；
            // 非整数、NaN、±Infinity 一律 false——规范 IsLooselyEqual
            // 的 BigInt/Number 分支）
            if let Some(HeapObject::BigInt(text)) = heap.get(r.0 as usize) {
                let t = text.trim();
                if n.is_nan() || n.is_infinite() || n.fract() != 0.0 {
                    return false;
                }
                return t == crate::ops::js_number_to_string(n);
            }
            // 包装实例（[[NumberValue]]/[[BooleanValue]] 数据槽）：解包比较
            if let Some(w) = wrapper_data(heap, r.0) {
                return match w.case() {
                    ValueCase::Number(w2) => n == w2,
                    ValueCase::Boolean(b) => n == f64::from(u8::from(b)),
                    _ => false,
                };
            }
            if let Some(s) = get_string_repr(r.0 as usize, heap, constants) {
                if let Ok(sn) = s.trim().parse::<f64>() {
                    return n == sn;
                }
            }
            false
        }
        (ValueCase::Boolean(b), ValueCase::Object(r))
        | (ValueCase::Object(r), ValueCase::Boolean(b)) => {
            // `true == new Boolean(true)`：包装解包后比较
            if let Some(w) = wrapper_data(heap, r.0) {
                return match w.case() {
                    ValueCase::Number(n2) => f64::from(u8::from(b)) == n2,
                    ValueCase::Boolean(b2) => b == b2,
                    _ => false,
                };
            }
            false
        }
        (ValueCase::Object(a), ValueCase::Object(b)) => {
            if a == b {
                true
            }
            // 包装实例解包一层后递归（`new String("hi") == "hi"`：
            // [[StringValue]] 持堆字符串 → 与另一侧 repr 比较）
            else if let Some(w) = wrapper_data(heap, a.0) {
                eq(*w, right, heap, constants)
            } else if let Some(w) = wrapper_data(heap, b.0) {
                eq(left, *w, heap, constants)
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
    match (left.case(), right.case()) {
        (ValueCase::Number(a), ValueCase::Number(b)) => a == b,
        (ValueCase::Boolean(a), ValueCase::Boolean(b)) => a == b,
        (ValueCase::Null, ValueCase::Null) | (ValueCase::Undefined, ValueCase::Undefined) => true,
        (ValueCase::Object(a), ValueCase::Object(b)) => {
            if a == b {
                true
            } else {
                // BigInt 按归一化十进制内容比较（`0b0_1n === 0b01n`）
                if let (Some(ba), Some(bb)) = (
                    get_bigint_repr(a.0 as usize, heap),
                    get_bigint_repr(b.0 as usize, heap),
                ) {
                    return ba == bb;
                }
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

/// BigInt 二元算术运算种类（`+` 混算拦截在 add_values 内单源）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BigIntArith {
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

/// BigInt 位运算种类（`& | ^ << >>`；`~` 走 [`Vm::bigint_bit_not`]）。
pub(crate) enum BigIntBit {
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

impl Vm {
    /// ECMAScript ToBoolean（借助本 VM 堆判定字符串内容）。
    #[must_use]
    pub fn truthy(&self, val: Value) -> bool {
        to_boolean(val, &self.heap)
    }

    /// 执行加法运算（支持数值相加与 ECMAScript 字符串自动拼接）。
    pub fn add_values(&mut self, left: Value, right: Value) -> Result<Value, VmError> {
        // ToPrimitive 快路径：包装对象（`new Boolean/Number/String`）解包为
        // 原始值。**Date 不在此列**——其 hint default 走 toString 得日期串
        //（`date + 1` 是字符串拼接；解包成 _timeValue 会得数字，S11.6.1_A2.2_T2）
        let unwrap_wrapper = |vm: &Self, v: Value| -> Value {
            if v.as_object()
                .is_some_and(|r| vm.has_own_slot(r.index(), "_isDate"))
            {
                return v;
            }
            vm.wrapper_primitive(v).unwrap_or(v)
        };
        let left = unwrap_wrapper(self, left);
        let right = unwrap_wrapper(self, right);
        // ToPrimitive(hint default)：`+` 的规范 hint 是 default——先
        // @@toPrimitive（hint "default"），再 Date→toString / 其余 valueOf→toString
        //（`date + 1` 得日期串、`{[Symbol.toPrimitive]:h=>h} + ""` 得 "default"）
        let left = self.to_primitive_default(left)?;
        let right = self.to_primitive_default(right)?;
        if let (ValueCase::Number(a), ValueCase::Number(b)) = (left.case(), right.case()) {
            return Ok(Value::Number(a + b));
        }
        let is_left_str = if let Some(r) = left.as_object() {
            matches!(self.heap.get(r.0 as usize), Some(HeapObject::String(_)))
        } else {
            false
        };
        let is_right_str = if let Some(r) = right.as_object() {
            matches!(self.heap.get(r.0 as usize), Some(HeapObject::String(_)))
        } else {
            false
        };
        // Buffer 参与 `+`：ToPrimitive 走 toString（utf8 内容），Node 语义
        // `'' + Buffer.from('ok')` → "ok"（而非 "[object Object]"）。
        let is_left_buf = self.is_buffer_value(left);
        let is_right_buf = self.is_buffer_value(right);

        // 规范序：任一侧为字符串 → 先走 ToString 拼接（`1n + "1"` === "11"，
        // 字符串分支在 BigInt 混算拦截**之前**）
        if is_left_str || is_right_str || is_left_buf || is_right_buf {
            // ToString(Symbol) 禁止：字符串拼接遇 Symbol → TypeError
            //（`'' + Symbol()` / 模板串插值；Node 22 实测）
            if self.is_symbol(left) || self.is_symbol(right) {
                return Err(self.type_error("Cannot convert a Symbol value to a string"));
            }
            let s1 = self.value_as_concat_text(left);
            let s2 = self.value_as_concat_text(right);
            let combined = format!("{s1}{s2}");
            let s_ref = self.alloc_string(combined);
            return Ok(Value::Object(s_ref));
        }
        // BigInt 运算族：单侧 BigInt → TypeError（规范禁止隐式混算——
        // `1n + true`/`Infinity + 1n` 等此前落入数值/拼接路径静默出错）；
        // 双侧 BigInt → 十进制大数加法（此前 `1n + 2n` 得 "12"——字符串
        // 连接而非算术）
        let (lb, rb) = (self.bigint_text(&left), self.bigint_text(&right));
        if lb.is_some() != rb.is_some() {
            return Err(
                self.type_error("Cannot mix BigInt and other types, use explicit conversions")
            );
        }
        if let (Some(lb), Some(rb)) = (lb, rb) {
            let dec = crate::bigdec::bigint_dec_add(&lb, &rb);
            let b_ref = self.alloc_bigint(dec);
            return Ok(Value::Object(b_ref));
        }
        // 双方都不是数值：任一为对象 → ToPrimitive 后字符串拼接
        // （`[] + []` === ""、`[] + {}` === "[object Object]"；生成语料实测）
        let left_obj = matches!(left.case(), ValueCase::Object(_));
        let right_obj = matches!(right.case(), ValueCase::Object(_));
        if left_obj || right_obj {
            let s1 = self.value_as_concat_text(left);
            let s2 = self.value_as_concat_text(right);
            let s_ref = self.alloc_string(format!("{s1}{s2}"));
            return Ok(Value::Object(s_ref));
        }

        // 双方皆原始值且非字符串 → **数值相加**（规范 `+` 的最后一步：
        // ToNumeric 后相加）。此前直接落到 `NaN`，导致
        // `true + 1` → NaN（Node 2）、`true + true` → NaN（Node 2）、
        // `null + 1` → NaN（Node 1）——布尔/null 参与算术的常见写法全错。
        // `undefined + 1` 仍为 NaN（规范如此）。
        Ok(Value::Number(
            self.to_number_value(left) + self.to_number_value(right),
        ))
    }

    /// ToPrimitive(hint number)：对象按 valueOf → toString 序解包出原始值；
    /// Date 实例 toString 优先（规范 hint string 特例）；皆非原始 → TypeError。
    /// 堆字符串/BigInt 虽为 Object case（NaN-box 堆形态），但语义是原始值——
    /// 直接返回（否则对堆字符串调 valueOf 会误触 String 原型占位）。
    ///
    /// 命名偏离 to_* 惯例以规避 wrong_self_convention（&mut self 为必需）。
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_primitive_number(&mut self, v: Value) -> Result<Value, VmError> {
        if let Some(r) = v.as_object() {
            if matches!(
                self.heap.get(r.0 as usize),
                Some(HeapObject::String(_))
                    | Some(HeapObject::BigInt(_))
                    | Some(HeapObject::Symbol { .. })
            ) {
                // Symbol：ToPrimitive 经 @@toPrimitive 返回**符号本身**
                //（不得调用 valueOf/toString——ToString(Symbol) 由调用方
                // 判 TypeError）
                return Ok(v);
            }
        }
        if !matches!(v.case(), ValueCase::Object(_)) {
            return Ok(v);
        }
        // @@toPrimitive：对象自定义转换协议优先于 valueOf/toString
        // （`{[Symbol.toPrimitive]: h => h}`；hint 为 "number"/"string"/"default"）
        if let Some(p) = self.call_to_primitive(v, "number")? {
            return Ok(p);
        }
        let r = v.as_object().expect("原始值早退后必为堆对象");
        // hint number：Date 例外——其 [[DefaultValue]] 走 hint string
        //（`date + 1` 得日期串而非时间值；S11.6.1_A2.2_T2 族）
        let is_date = self.has_own_slot(r.0 as usize, "_isDate");
        let (first, second) = if is_date {
            ("toString", "valueOf")
        } else {
            ("valueOf", "toString")
        };
        for m in [first, second] {
            let mv = self.get_property(v, m)?;
            let callable = mv.as_object().is_some_and(|f| {
                matches!(
                    self.heap.get(f.0 as usize),
                    Some(HeapObject::Closure { .. })
                        | Some(HeapObject::NativeFn { .. })
                        | Some(HeapObject::NativeCtor { .. })
                )
            });
            if callable {
                let res = self.invoke_callable(mv, v, &[])?;
                // 结果是原始值即采纳；NaN-box 下堆字符串/BigInt 亦为 Object
                // case（语义是原始值），须与开头的早退口径一致
                let res_primitive = match res.case() {
                    ValueCase::Object(r) => matches!(
                        self.heap.get(r.0 as usize),
                        Some(HeapObject::String(_))
                            | Some(HeapObject::BigInt(_))
                            | Some(HeapObject::Symbol { .. })
                    ),
                    _ => true,
                };
                if res_primitive {
                    return Ok(res);
                }
            }
        }
        // 规范 TypeError（V8 同文案）：实例挂 TypeError.prototype +
        // name（prims.rs syntax_error 同口径，instanceof/constructor 判型）
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    /// 为手拼的错误实例挂对应子类原型并覆盖自有 `name`
    /// （alloc_error_instance 预置自有 name="Error"，不覆盖会遮蔽子类
    /// 原型名；`instanceof TypeError` 等判型语义；typed_error 之外的
    /// 散布构造点统一入口）。
    pub(crate) fn attach_error_proto(&mut self, err: ObjectRef, ctor_name: &str) {
        let ctor = self.error_subclass_ctor(ctor_name);
        if let Some(ValueCase::Object(p)) =
            self.get_property(ctor, "prototype").ok().map(|v| v.case())
        {
            self.set_prototype_of(Value::Object(err), Some(p));
        }
        // `name` 挂在**子类原型**上（Node：`new TypeError('t').name` 读自
        // `TypeError.prototype.name`）——实例不落自有 name，否则
        // `Object.getOwnPropertyNames(err)` 多出一项、与 Node 不符。
        // 子类原型的 name 已由 `error_subclass_ctor` 建好；此处只兜底
        //（原型缺 name 时才补，避免无谓写入）。
        let proto_has_name = if let Some(ValueCase::Object(p)) =
            self.get_property(ctor, "prototype").ok().map(|v| v.case())
        {
            self.has_own_slot(p.0 as usize, "name")
        } else {
            false
        };
        if !proto_has_name {
            let name = self.alloc_string(ctor_name.to_owned());
            if let Some(ValueCase::Object(p)) =
                self.get_property(ctor, "prototype").ok().map(|v| v.case())
            {
                let _ = self.set_property(Value::Object(p), "name", Value::Object(name));
                self.mark_non_enumerable(Value::Object(p), "name");
            }
        }
        // 子类改名后同步 stack 首行（`TypeError: msg`），并统一收口可枚举性
        // （Node：`Object.keys(err)` 为空集）
        self.refresh_error_stack_name(err);
        self.refresh_error_enumerability(err);
    }

    /// 构造带对应原型 + `name` 的 Error 抛出值（TypeError/RangeError 等；
    /// instanceof / `constructor` 判型对官方 assert.throws 必需）。
    pub(crate) fn typed_error(&mut self, ctor_name: &str, msg: &str) -> VmError {
        let ctor = self.error_subclass_ctor(ctor_name);
        let err = self.alloc_error_instance(msg);
        // name 挂在子类原型（`error_subclass_ctor` 已建；实例不落自有 name）
        if let Some(ValueCase::Object(p)) =
            self.get_property(ctor, "prototype").ok().map(|v| v.case())
        {
            self.set_prototype_of(Value::Object(err), Some(p));
        }
        // 子类名已就位：同步 stack 首行 + 统一收口可枚举性
        self.refresh_error_stack_name(err);
        self.refresh_error_enumerability(err);
        VmError::Thrown(Value::Object(err))
    }

    /// 构造带 `TypeError.prototype` + `name` 的 TypeError 抛出值。
    pub(crate) fn type_error(&mut self, msg: &str) -> VmError {
        self.typed_error("TypeError", msg)
    }

    /// 二元算术（`-` `*` `/` `%` `**`）统一收口：两侧均 BigInt → BigInt
    /// 结果；均非 BigInt → 直接完成数值运算；**单侧 BigInt** → TypeError
    /// （规范禁止隐式混算）。对象参数先 ToPrimitive（valueOf 产 BigInt
    /// 采纳——bigint-toprimitive 族）。
    ///
    /// 规范 ToNumeric 顺序：ToPrimitive(lhs) 及其 Symbol 拦截**完成于**
    /// rhs 的 ToPrimitive 之前（order-of-evaluation 族——lhs valueOf
    /// 抛错/返回 symbol 时 rhs valueOf 不得被调用）；且双侧非 BigInt 时
    /// 在此直接出数值结果，调用方不重入 ToPrimitive（否则用户 valueOf
    /// 会被二次调用）。
    pub(crate) fn bigint_binary(
        &mut self,
        left: Value,
        right: Value,
        op: BigIntArith,
    ) -> Result<Value, VmError> {
        // wrapper 内部槽直解先于 ToPrimitive（r15 同教训：wrapper 的
        // valueOf 占位会被 invoke_callable 误调用）
        let left = self.wrapper_primitive(left).unwrap_or(left);
        let right = self.wrapper_primitive(right).unwrap_or(right);
        let lprim = self.to_primitive_number(left)?;
        if self.is_symbol(lprim) {
            return Err(self.type_error("Cannot convert a Symbol value to a number"));
        }
        let rprim = self.to_primitive_number(right)?;
        if self.is_symbol(rprim) {
            return Err(self.type_error("Cannot convert a Symbol value to a number"));
        }
        let lb = self.bigint_text(&lprim);
        let rb = self.bigint_text(&rprim);
        let (lb, rb) = match (lb, rb) {
            (Some(l), Some(r)) => (l, r),
            (None, None) => {
                let a = self.to_number_value(lprim);
                let b = self.to_number_value(rprim);
                return Ok(Value::Number(match op {
                    BigIntArith::Sub => a - b,
                    BigIntArith::Mul => a * b,
                    BigIntArith::Div => a / b,
                    BigIntArith::Mod => a % b,
                    BigIntArith::Pow => a.powf(b),
                }));
            }
            _ => {
                return Err(
                    self.type_error("Cannot mix BigInt and other types, use explicit conversions")
                );
            }
        };
        let text = match op {
            BigIntArith::Sub => crate::bigdec::bigint_dec_sub(&lb, &rb),
            BigIntArith::Mul => crate::bigdec::bigint_dec_mul(&lb, &rb),
            BigIntArith::Div | BigIntArith::Mod => {
                let Some((q, r)) = crate::bigdec::bigint_dec_divmod(&lb, &rb) else {
                    return Err(self.typed_error("RangeError", "Division by zero"));
                };
                if op == BigIntArith::Div { q } else { r }
            }
            BigIntArith::Pow => {
                let Some(text) = crate::bigdec::bigint_dec_pow(&lb, &rb) else {
                    return Err(self.typed_error("RangeError", "undefined must be positive"));
                };
                text
            }
        };
        Ok(Value::Object(self.alloc_bigint(text)))
    }

    /// BigInt 位运算（`& | ^ << >>`）：两操作数均须为 BigInt（任一侧为
    /// BigInt 时分派至此；混用数值按规范 TypeError）。CBOR 编解码、
    /// uuidv7 等真实包的顶层初始化依赖（`1n << 41n` 此前误走数值
    /// ToNumber 通道抛 TypeError）。
    pub(crate) fn bigint_bitwise(
        &mut self,
        left: Value,
        right: Value,
        op: BigIntBit,
    ) -> Result<Value, VmError> {
        use crate::bigdec::{BigIntBitKind, bigint_bit_op, bigint_shift};
        let left = self.wrapper_primitive(left).unwrap_or(left);
        let right = self.wrapper_primitive(right).unwrap_or(right);
        let lprim = self.to_primitive_number(left)?;
        if self.is_symbol(lprim) {
            return Err(self.type_error("Cannot convert a Symbol value to a number"));
        }
        let rprim = self.to_primitive_number(right)?;
        if self.is_symbol(rprim) {
            return Err(self.type_error("Cannot convert a Symbol value to a number"));
        }
        let (lb, rb) = match (self.bigint_text(&lprim), self.bigint_text(&rprim)) {
            (Some(l), Some(r)) => (l, r),
            _ => {
                return Err(
                    self.type_error("Cannot mix BigInt and other types, use explicit conversions")
                );
            }
        };
        let text = match op {
            BigIntBit::And => bigint_bit_op(&lb, &rb, BigIntBitKind::And),
            BigIntBit::Or => bigint_bit_op(&lb, &rb, BigIntBitKind::Or),
            BigIntBit::Xor => bigint_bit_op(&lb, &rb, BigIntBitKind::Xor),
            BigIntBit::Shl => bigint_shift(&lb, self.shift_amount(&rb)?, false)
                .ok_or_else(|| self.typed_error("RangeError", "Maximum BigInt size exceeded"))?,
            BigIntBit::Shr => bigint_shift(&lb, self.shift_amount(&rb)?, true)
                .ok_or_else(|| self.typed_error("RangeError", "Maximum BigInt size exceeded"))?,
        };
        Ok(Value::Object(self.alloc_bigint(text)))
    }

    /// 移位量解析：BigInt 十进制串 → u64（负数/超界 → RangeError）。
    fn shift_amount(&mut self, text: &str) -> Result<u64, VmError> {
        let t = text.trim().trim_start_matches('+');
        let (neg, digits) = match t.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, t),
        };
        let mut acc: u64 = 0;
        for b in digits.bytes() {
            if !b.is_ascii_digit() {
                continue;
            }
            acc = acc
                .checked_mul(10)
                .and_then(|v| v.checked_add((b - b'0') as u64))
                .ok_or_else(|| self.typed_error("RangeError", "Maximum BigInt size exceeded"))?;
            if acc > (1 << 26) {
                return Err(self.typed_error("RangeError", "Maximum BigInt size exceeded"));
            }
        }
        if neg && acc != 0 {
            return Err(self.typed_error("RangeError", "Maximum BigInt size exceeded"));
        }
        Ok(acc)
    }

    /// `~a`（BigInt 按位非 = -a-1）。
    pub(crate) fn bigint_bit_not(&mut self, v: Value) -> Result<Value, VmError> {
        let prim = self.wrapper_primitive(v).unwrap_or(v);
        let p = self.to_primitive_number(prim)?;
        let Some(text) = self.bigint_text(&p) else {
            return Err(
                self.type_error("Cannot mix BigInt and other types, use explicit conversions")
            );
        };
        Ok(Value::Object(
            self.alloc_bigint(crate::bigdec::bigint_bit_not(&text)),
        ))
    }

    /// 二元/一元数值算子（`-` `*` `/` `%` `**` 位运算、一元 ±）的操作数
    /// 准备：先解包装对象/Date 内部槽（与 add_values 快路径同序——须在
    /// ToPrimitive 之前，否则 wrapper 的 valueOf 占位会被误调用），再
    /// ToPrimitive(hint number)（用户 valueOf/toString 可抛错，故带错误
    /// 通道），最后取数值。
    ///
    /// 命名偏离 to_* 惯例以规避 wrong_self_convention（&mut self 为必需）。
    pub(crate) fn numeric_operand(&mut self, v: Value) -> Result<f64, VmError> {
        // 包装对象（new Number/String/Boolean）与 Date：内部槽直解
        //（含 _timeValue——Date 算术 hint number 即时间值）
        if let Some(w) = self.wrapper_primitive(v) {
            return Ok(self.to_number_value(w));
        }
        let p = self.to_primitive_number(v)?;
        // ToNumber(Symbol) → TypeError（symbol 在 to_primitive_number 中
        // 原样返回，此处须拦——`Number(Symbol())`/`+Symbol()` 均抛）
        if self.is_symbol(p) {
            return Err(self.type_error("Cannot convert a Symbol value to a number"));
        }
        // ToNumber(BigInt) → TypeError（`+1n` / `Math.max(1n)` 均抛；
        // 此前经 to_number_value 静默转数值）
        if self.is_bigint_value(p) {
            return Err(self.type_error("Cannot convert a BigInt value to a number"));
        }
        Ok(self.to_number_value(p))
    }

    /// `@@toPrimitive`（`Symbol.toPrimitive`）协议：对象定义该方法时按其
    /// 结果作为 ToPrimitive 输出（返回非原始值 → TypeError）；未定义 → None
    /// （调用方回退 valueOf/toString 序）。
    pub(crate) fn call_to_primitive(
        &mut self,
        v: Value,
        hint: &str,
    ) -> Result<Option<Value>, VmError> {
        // 惰性物化知名符号（&mut self 可用）：缓存未命中时首次创建——
        // 否则 `o[Symbol.toPrimitive] = fn` 的协议永远查不到键
        let sym = self.well_known_symbol("toPrimitive");
        let sym_ref = sym.as_object().expect("知名符号必为堆对象");
        let key = crate::symbol::mangled_key(sym_ref);
        let f = self.get_property(v, &key)?;
        // GetMethod 语义：undefined **与 null** 均视为缺失（回退
        // valueOf/toString 序）；存在但**非可调用**（`{[Symbol.toPrimitive]: 1}`）
        // 才按规范 ToPrimitive 抛 TypeError（BigInt 混算族）
        if f.is_undefined() || f.is_null() {
            return Ok(None);
        }
        let callable = f.as_object().is_some_and(|o| {
            matches!(
                self.heap.get(o.index()),
                Some(HeapObject::Closure { .. })
                    | Some(HeapObject::NativeFn { .. })
                    | Some(HeapObject::NativeCtor { .. })
            )
        });
        if !callable {
            return Err(self.type_error("Symbol.toPrimitive is not a function"));
        }
        let hint_val = self.alloc_string(hint.to_owned());
        let res = self.invoke_callable(f, v, &[Value::Object(hint_val)])?;
        let primitive = match res.case() {
            ValueCase::Object(rr) => matches!(
                self.heap.get(rr.index()),
                Some(HeapObject::String(_))
                    | Some(HeapObject::BigInt(_))
                    | Some(HeapObject::Symbol { .. })
            ),
            _ => true,
        };
        if !primitive {
            return Err(self.type_error("Cannot convert object to primitive value"));
        }
        Ok(Some(res))
    }

    /// ToPrimitive(hint default)：`+` 运算符与 `==` 用——先 `@@toPrimitive`
    /// （hint "default"，Date 在协议内部走 hint string），再按 Date 特例
    /// 取 toString、其余取 valueOf → toString。
    ///
    /// 命名偏离 to_* 惯例以规避 wrong_self_convention（&mut self 为必需）。
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_primitive_default(&mut self, v: Value) -> Result<Value, VmError> {
        if !matches!(v.case(), ValueCase::Object(_)) {
            return Ok(v);
        }
        if let Some(p) = self.call_to_primitive(v, "default")? {
            return Ok(p);
        }
        if let Some(r) = v.as_object()
            && matches!(
                self.heap.get(r.index()),
                Some(HeapObject::String(_))
                    | Some(HeapObject::BigInt(_))
                    | Some(HeapObject::Symbol { .. })
            )
        {
            return Ok(v);
        }
        let r = v.as_object().expect("对象分支");
        let is_date = self.has_own_slot(r.index(), "_isDate");
        let (first, second) = if is_date {
            ("toString", "valueOf")
        } else {
            ("valueOf", "toString")
        };
        for m in [first, second] {
            let mv = self.get_property(v, m)?;
            let callable = mv.as_object().is_some_and(|f| {
                matches!(
                    self.heap.get(f.index()),
                    Some(HeapObject::Closure { .. })
                        | Some(HeapObject::NativeFn { .. })
                        | Some(HeapObject::NativeCtor { .. })
                )
            });
            if callable {
                let res = self.invoke_callable(mv, v, &[])?;
                let primitive = match res.case() {
                    ValueCase::Object(rr) => matches!(
                        self.heap.get(rr.index()),
                        Some(HeapObject::String(_))
                            | Some(HeapObject::BigInt(_))
                            | Some(HeapObject::Symbol { .. })
                    ),
                    _ => true,
                };
                if primitive {
                    return Ok(res);
                }
            }
        }
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    /// ToString（hint string，全语义）：对象按 `toString` → `valueOf` 序
    /// 取原始值后转串（用户自定义 toString 生效——`String({toString(){...}})`
    /// / `new String(obj)`）；数组/Date 等经其原型方法；皆非原始 → TypeError。
    pub(crate) fn js_string(&mut self, v: Value) -> Result<String, VmError> {
        // 堆字符串：直接取文本
        if let Some(r) = v.as_object()
            && let Some(HeapObject::String(s)) = self.heap.get(r.index())
        {
            return Ok(s.clone());
        }
        // 包装对象内部槽直解（须先于 ToPrimitive，避免 valueOf 占位误调用）
        if let Some(w) = self.wrapper_primitive(v) {
            return self.js_string(w);
        }
        if !matches!(v.case(), ValueCase::Object(_)) {
            return Ok(self.format_value(v));
        }
        // `String(sym)` 是规范中**唯一**允许符号转串的路径
        // （SymbolDescriptiveString → "Symbol(desc)"）；`"" + sym` 由
        // add_values 字符串分支单独拦截抛 TypeError
        if self.is_symbol(v) {
            return Ok(self.format_value(v));
        }
        // @@toPrimitive（hint "string"）优先于 toString/valueOf 序
        if let Some(p) = self.call_to_primitive(v, "string")? {
            if self.is_symbol(p) {
                return Ok(self.format_value(p));
            }
            return Ok(self.format_value(p));
        }
        // hint string：toString → valueOf（Date 同序——hint string 下
        // Date.prototype.toString 即日期可读形式）
        for m in ["toString", "valueOf"] {
            let mv = self.get_property(v, m)?;
            let callable = mv.as_object().is_some_and(|f| {
                matches!(
                    self.heap.get(f.index()),
                    Some(HeapObject::Closure { .. })
                        | Some(HeapObject::NativeFn { .. })
                        | Some(HeapObject::NativeCtor { .. })
                )
            });
            if callable {
                let res = self.invoke_callable(mv, v, &[])?;
                let primitive = match res.case() {
                    ValueCase::Object(rr) => matches!(
                        self.heap.get(rr.index()),
                        Some(HeapObject::String(_))
                            | Some(HeapObject::BigInt(_))
                            | Some(HeapObject::Symbol { .. })
                    ),
                    _ => true,
                };
                if primitive {
                    return Ok(self.format_value(res));
                }
            }
        }
        // 无可用方法：数组 join / 普通对象 "[object Object]" 兜底
        Ok(self.format_value(v))
    }

    /// 严格 ToString（规范 ToString 语义）：对象经 toString → valueOf 取原始值；
    /// **两者皆不可调用或皆不产原始值 → TypeError**（无 `[object Object]` 兜底）。
    /// 供 Symbol 描述转换等要求严格语义的调用点使用。
    pub(crate) fn js_string_strict(&mut self, v: Value) -> Result<String, VmError> {
        let is_heap_str = v
            .as_object()
            .is_some_and(|r| matches!(self.heap.get(r.index()), Some(HeapObject::String(_))));
        if !matches!(v.case(), ValueCase::Object(_)) || is_heap_str {
            return self.js_string(v);
        }
        // BigInt：堆对象但语义是**原始值**——ToString 即其十进制文本
        //（`String(1n)` === "1"；此前落入 toString/valueOf 查找路径因 BigInt
        // 无这些方法而抛 "Cannot convert object to primitive value"）
        if self.is_bigint_value(v) {
            return Ok(self.format_value(v));
        }
        if self.is_symbol(v) {
            // 严格 ToString：符号参数直接 TypeError（`Symbol.for(Symbol())`
            // / `Symbol(Symbol())`——描述串形态仅 `String(sym)` 走 js_string）
            return Err(self.type_error("Cannot convert a Symbol value to a string"));
        }
        for m in ["toString", "valueOf"] {
            let mv = self.get_property(v, m)?;
            let callable = mv.as_object().is_some_and(|f| {
                matches!(
                    self.heap.get(f.index()),
                    Some(HeapObject::Closure { .. })
                        | Some(HeapObject::NativeFn { .. })
                        | Some(HeapObject::NativeCtor { .. })
                )
            });
            if !callable {
                continue;
            }
            let res = self.invoke_callable(mv, v, &[])?;
            let primitive = match res.case() {
                ValueCase::Object(rr) => matches!(
                    self.heap.get(rr.index()),
                    Some(HeapObject::String(_))
                        | Some(HeapObject::BigInt(_))
                        | Some(HeapObject::Symbol { .. })
                ),
                _ => true,
            };
            if primitive {
                // 结果为符号：ToString(Symbol) 禁止（调用方按需特殊处理）
                if self.is_symbol(res) {
                    return Err(self.type_error("Cannot convert a Symbol value to a string"));
                }
                return Ok(self.format_value(res));
            }
        }
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    /// `BigInt(v)`：数字须为整数（否则 RangeError）、字符串按字面量解析
    /// （含 0x/0b/0o 前缀与空白裁剪）、布尔 → 0n/1n、BigInt 原样。
    pub(crate) fn bigint_from_value(&mut self, v: Value) -> Result<Value, VmError> {
        match v.case() {
            ValueCase::Object(r) => match self.heap.get(r.index()) {
                Some(HeapObject::BigInt(_)) => Ok(v),
                Some(HeapObject::String(s)) => {
                    let t = s.trim();
                    if t.is_empty() {
                        return Ok(Value::Object(self.alloc_bigint("0".to_owned())));
                    }
                    let norm = crate::bigdec::normalize_bigint_literal(t);
                    // 归一化后仍含非数字字符 → 语法错误（SyntaxError）
                    let digits = norm.strip_prefix('-').unwrap_or(&norm);
                    let ok = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
                    if !ok {
                        return Err(self.typed_error(
                            "SyntaxError",
                            &format!("Cannot convert {s} to a BigInt"),
                        ));
                    }
                    Ok(Value::Object(self.alloc_bigint(norm)))
                }
                _ => Err(self.type_error("Cannot convert object to a BigInt")),
            },
            ValueCase::Number(n) => {
                if !n.is_finite() || n.fract() != 0.0 {
                    return Err(self.typed_error(
                        "RangeError",
                        "The number cannot be converted to a BigInt because it is not an integer",
                    ));
                }
                Ok(Value::Object(self.alloc_bigint(format!("{}", n as i64))))
            }
            ValueCase::Boolean(b) => Ok(Value::Object(
                self.alloc_bigint(if b { "1" } else { "0" }.to_owned()),
            )),
            _ => Err(self.type_error("Cannot convert undefined or null to a BigInt")),
        }
    }

    /// 值为 BigInt 堆对象时取其十进制文本。
    fn bigint_text(&self, v: &Value) -> Option<String> {
        match v.case() {
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::BigInt(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// 值是否为 Buffer 实例（`_isBuffer` 标记）。
    fn is_buffer_value(&self, v: Value) -> bool {
        matches!(v.case(), ValueCase::Object(r) if self.has_own_slot(r.0 as usize, "_isBuffer"))
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
        match v.case() {
            ValueCase::Number(n) => CmpPrimitive::Num(n),
            ValueCase::Boolean(_) | ValueCase::Null | ValueCase::Undefined => {
                CmpPrimitive::Num(to_number(v))
            }
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(s)) => CmpPrimitive::Str(s.clone()),
                Some(HeapObject::BigInt(b)) => {
                    CmpPrimitive::Num(b.trim().parse::<f64>().unwrap_or(f64::NAN))
                }
                _ => match self.own_value(r.0 as usize, "_timeValue").map(|v| v.case()) {
                    // Date 实例：valueOf 产原始数值 → 按时间值比较
                    Some(ValueCase::Number(t)) => CmpPrimitive::Num(t),
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
