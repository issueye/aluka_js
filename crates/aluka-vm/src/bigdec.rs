//! 数字格式化底层：double 的**精确十进制展开**（极简 bignum，零外部依赖）＋
//! `toString(radix)` 的 V8 算法逐句移植。
//!
//! Node.js（V8）的 `Number.prototype.toFixed` / `toExponential` / `toPrecision`
//! 以 double 的**精确**十进制展开为基准做**字符串舍入**：规范要求「取最接近 x 的
//! n，并列时取**较大**者」＝五入，而 Rust 的 `format!("{:.n}")` 是四舍六入五取偶
//! （banker's）：`(0.5).toFixed(0)` 得 `"0"` 而规范要 `"1"`；同时 double 的整数
//! 部分可达 1.8e308（远超 `i128`），故整数位展开必须自带 bignum。
//!
//! 而 `toString(radix)` 恰恰相反：V8 的 `DoubleToRadixCString` 全程用 f64 算术，
//! 大数的低位是**故意补零**的结果，因此这里逐句移植该算法而非用精确展开。
//!
//! 因此本模块提供：
//! - [`BigNat`]：基 2^32 小端无符号大整数，仅覆盖十进制展开所需运算（无依赖）；
//! - [`exact_decimal`]：double → 精确十进制数字串 + 小数点位置；
//! - [`to_fixed`] / [`to_exponential`] / [`to_precision`] / [`to_radix_string`]：
//!   四个格式化纯函数（参数区间校验与 RangeError 由调用方 `surface.rs` 负责）。
//!
//! 精确展开下「并列」判定是平凡的：被切掉的首位是 `'5'` 且其后无有效数字
//! （数字串已去尾零），故按首位 `>= '5'` 一律进位即五入，无需浮点比较。

/// 进制数字表（2~36，小写）。
const RADIX_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// 十进制展开的分块除数（`divmod_small` 单字上限 2^32-1）。
const DEC_CHUNK: u32 = 1_000_000_000;

/// 基 2^32 小端无符号大整数（仅本模块所需的极简实现，非通用大数库）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BigNat {
    /// 小端 32 位字；规范化为「最高字非零」（零值为空表）。
    words: Vec<u32>,
}

impl BigNat {
    /// 由 `u64` 构造。
    pub(crate) fn from_u64(v: u64) -> Self {
        let mut words = vec![v as u32];
        if v >> 32 != 0 {
            words.push((v >> 32) as u32);
        }
        Self { words }
    }

    /// 是否为零。
    pub(crate) fn is_zero(&self) -> bool {
        self.words.is_empty()
    }

    /// `self *= m`（`m` 为单字乘数，逐字 64 位累加）。
    pub(crate) fn mul_small(&mut self, m: u32) {
        let mut carry: u64 = 0;
        for w in &mut self.words {
            let v = u64::from(*w) * u64::from(m) + carry;
            *w = v as u32;
            carry = v >> 32;
        }
        while carry != 0 {
            self.words.push(carry as u32);
            carry >>= 32;
        }
    }

    /// `self <<= bits`（先整字平移再位内平移）。
    fn shl(&mut self, bits: u32) {
        if self.is_zero() || bits == 0 {
            return;
        }
        let word_shift = (bits / 32) as usize;
        let bit_shift = bits % 32;
        if bit_shift > 0 {
            let mut carry = 0u32;
            for w in &mut self.words {
                let v = u64::from(*w) << bit_shift;
                *w = (v as u32) | carry;
                carry = (v >> 32) as u32;
            }
            if carry != 0 {
                self.words.push(carry);
            }
        }
        if word_shift > 0 {
            let mut shifted = vec![0u32; word_shift];
            shifted.extend_from_slice(&self.words);
            self.words = shifted;
        }
    }

    /// `self /= d`（`d != 0`），返回余数；商就地规范化（去高位零字）。
    pub(crate) fn divmod_small(&mut self, d: u32) -> u32 {
        let mut rem: u64 = 0;
        for w in self.words.iter_mut().rev() {
            let cur = (rem << 32) | u64::from(*w);
            *w = (cur / u64::from(d)) as u32;
            rem = cur % u64::from(d);
        }
        while self.words.last() == Some(&0) {
            self.words.pop();
        }
        rem as u32
    }

    /// 十进制字符串（无前导零；零值 → `"0"`）：反复 `divmod_small(10^9)` 取块。
    pub(crate) fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let mut chunks: Vec<u32> = Vec::new();
        let mut cur = self.clone();
        while !cur.is_zero() {
            chunks.push(cur.divmod_small(DEC_CHUNK));
        }
        // 最高块不补零，其余每块定宽 9 位
        let mut out = chunks.pop().map_or_else(String::new, |c| c.to_string());
        for c in chunks.iter().rev() {
            out.push_str(&format!("{c:09}"));
        }
        out
    }

    /// `self += d`（`d < 32` 单字加数——十进制按位折叠用）。
    pub(crate) fn add_small(&mut self, d: u32) {
        let mut carry = d;
        for w in &mut self.words {
            let v = u64::from(*w) + u64::from(carry);
            *w = v as u32;
            carry = (v >> 32) as u32;
            if carry == 0 {
                return;
            }
        }
        if carry != 0 {
            self.words.push(carry);
        }
    }

    /// `self += other`（逐字加 + 进位）。
    pub(crate) fn add_big(&mut self, other: &BigNat) {
        let n = self.words.len().max(other.words.len());
        let mut carry: u64 = 0;
        for i in 0..n {
            let a = u64::from(self.words.get(i).copied().unwrap_or(0));
            let b = u64::from(other.words.get(i).copied().unwrap_or(0));
            let v = a + b + carry;
            if i < self.words.len() {
                self.words[i] = v as u32;
            } else {
                self.words.push(v as u32);
            }
            carry = v >> 32;
        }
        if carry != 0 {
            self.words.push(carry as u32);
        }
    }

    /// `self -= other`（要求 `self >= other`，借位逐字传播后规范化）。
    pub(crate) fn sub_big(&mut self, other: &BigNat) {
        let mut borrow: i64 = 0;
        for i in 0..self.words.len() {
            let a = i64::from(self.words[i]);
            let b = i64::from(other.words.get(i).copied().unwrap_or(0));
            let mut v = a - b - borrow;
            if v < 0 {
                v += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.words[i] = v as u32;
        }
        while self.words.last() == Some(&0) {
            self.words.pop();
        }
    }

    /// 绝对值比较：`Greater` = `self > other`。
    pub(crate) fn cmp_big(a: &BigNat, b: &BigNat) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match a.words.len().cmp(&b.words.len()) {
            Ordering::Equal => {}
            o => return o,
        }
        for i in (0..a.words.len()).rev() {
            match a.words[i].cmp(&b.words[i]) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        Ordering::Equal
    }
}

/// BigInt 字面量载荷归一化：进制前缀（`0x`/`0b`/`0o` + 可选负号）→
/// 十进制串；已是十进制的原样返回（`alloc_bigint` 堆表示约定为十进制）。
pub(crate) fn normalize_bigint_literal(s: &str) -> String {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (radix, digits) =
        if let Some(d) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
            (16u32, d)
        } else if let Some(d) = rest.strip_prefix("0b").or_else(|| rest.strip_prefix("0B")) {
            (2u32, d)
        } else if let Some(d) = rest.strip_prefix("0o").or_else(|| rest.strip_prefix("0O")) {
            (8u32, d)
        } else {
            return s.to_owned();
        };
    let mut acc = BigNat::default();
    for ch in digits.chars() {
        if let Some(d) = ch.to_digit(radix) {
            acc.mul_small(radix);
            acc.add_small(d);
        }
    }
    if neg && !acc.is_zero() {
        format!("-{}", acc.to_decimal_string())
    } else {
        acc.to_decimal_string()
    }
}

/// 十进制字符串加法（BigInt 运行时运算；支持可选负号——同号相加/异号
/// 相减取大符；结果零规范为 "0"）。
pub(crate) fn bigint_dec_add(a: &str, b: &str) -> String {
    let (neg_a, mag_a) = strip_sign(a);
    let (neg_b, mag_b) = strip_sign(b);
    let mut x = dec_to_bignat(mag_a);
    let mut y = dec_to_bignat(mag_b);
    let neg = match (neg_a, neg_b) {
        (false, false) => false,
        (true, true) => true,
        (false, true) | (true, false) => {
            // 异号：|大| - |小|，符号随大者
            match BigNat::cmp_big(&x, &y) {
                std::cmp::Ordering::Equal => return "0".to_owned(),
                std::cmp::Ordering::Greater => neg_a,
                std::cmp::Ordering::Less => {
                    std::mem::swap(&mut x, &mut y);
                    neg_b
                }
            }
        }
    };
    if neg_a == neg_b {
        x.add_big(&y);
    } else {
        x.sub_big(&y);
    }
    if neg && !x.is_zero() {
        format!("-{}", x.to_decimal_string())
    } else {
        x.to_decimal_string()
    }
}

fn strip_sign(s: &str) -> (bool, &str) {
    match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    }
}

fn dec_to_bignat(s: &str) -> BigNat {
    let mut acc = BigNat::default();
    for ch in s.chars() {
        if let Some(d) = ch.to_digit(10) {
            acc.mul_small(10);
            acc.add_small(d);
        }
    }
    acc
}

/// double 的**精确**十进制展开：`值 = ±0.digits × 10^point`。
struct ExactDecimal {
    /// 符号（`n < 0`；`-0` 为 `false`，与 `Number::toString` 的符号规则一致）。
    neg: bool,
    /// 全部有效数字（已去尾随零；零值为 `[b'0']`）。
    digits: Vec<u8>,
    /// 小数点位置：`0.digits × 10^point`（即 `digits` 的十进制指数 +1）。
    point: i32,
}

/// 去掉十进制串的尾随零（至少保留一位）。
fn trim_zero_tail(s: &str) -> Vec<u8> {
    let mut digits = s.as_bytes().to_vec();
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    digits
}

/// double → 精确十进制展开：拆 `M · 2^E`（`M` 含隐式位）后
/// `E ≥ 0` 走 bignum 左移（整数），`E < 0` 走 `M · 5^k`（`k = -E`，小数位 = `k`）。
fn exact_decimal(n: f64) -> ExactDecimal {
    if n == 0.0 {
        return ExactDecimal {
            neg: false,
            digits: vec![b'0'],
            point: 1,
        };
    }
    let neg = n < 0.0;
    let bits = n.abs().to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & 0x000f_ffff_ffff_ffff;
    let (mantissa, exp2) = if raw_exp == 0 {
        // 次正规：无隐式前导位，指数固定 -1074
        (frac, -1074)
    } else {
        (frac | (1_u64 << 52), raw_exp - 1075)
    };
    let mut big = BigNat::from_u64(mantissa);
    if exp2 >= 0 {
        big.shl(exp2.unsigned_abs());
        let s = big.to_decimal_string();
        let point = s.len() as i32;
        ExactDecimal {
            neg,
            digits: trim_zero_tail(&s),
            point,
        }
    } else {
        let k = exp2.unsigned_abs();
        for _ in 0..k {
            big.mul_small(5);
        }
        let s = big.to_decimal_string();
        let point = s.len() as i32 - k as i32;
        ExactDecimal {
            neg,
            digits: trim_zero_tail(&s),
            point,
        }
    }
}

/// 最短可往返数字（借 Rust 的 `{:e}` 取最短尾数），返回 `digits` 与
/// `0.digits × 10^point` 的 `point`。**不含**规范要求的并列取偶，见
/// [`shortest_spec_digits`]。
fn shortest_digits(a: f64) -> (Vec<u8>, i32) {
    let e_form = format!("{a:e}");
    let (mant, exp) = e_form.split_once('e').unwrap_or((e_form.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mant.bytes().filter(|c| *c != b'.').collect();
    (trim_zero_tail(&String::from_utf8_lossy(&digits)), exp + 1)
}

/// 规范 [`Number::toString`] / `toExponential()`（无参）要求的数字串：
/// **最短**有效数字，且「最接近原值；两个候选同样接近时取**末位为偶**者」。
///
/// Rust `{:e}` 已给出最短且最接近的数字串，但并列时取**较大**者
/// （`1501199875790165.25` → `…653`），而 ECMA-262 6.1.6.1.20 要求取偶
/// （Node 实测为 `…652`）。这里借 [`exact_decimal`] 的精确展开判别并列并改取偶。
///
/// 注意与 [`to_fixed`] / [`to_precision`] 的规则**不同**：后两者规范明确要求
/// 并列时取**较大**者（Node 实测 `(1501199875790165.25).toPrecision(17)` 为
/// `"1501199875790165.3"`）。
pub(crate) fn shortest_spec_digits(a: f64) -> (String, i32) {
    let (digits, point) = shortest_digits(a);
    let exact = exact_decimal(a);
    let k = digits.len();
    // 并列判别：精确展开恰为「k 位 + 一个 '5'」→ 正好落在两个 k 位十进制正中
    if exact.digits.len() != k + 1 || exact.digits[k] != b'5' {
        return (String::from_utf8_lossy(&digits).into_owned(), point);
    }
    let lower = &exact.digits[..k];
    let mut chosen = lower.to_vec();
    let mut chosen_point = exact.point;
    if lower[k - 1] % 2 == 1 {
        // 末位为奇 → 改取相邻的偶候选（`lower + 1`，可能进出一位）
        let mut carry = true;
        for d in chosen.iter_mut().rev() {
            if !carry {
                break;
            }
            if *d == b'9' {
                *d = b'0';
            } else {
                *d += 1;
                carry = false;
            }
        }
        if carry {
            chosen.insert(0, b'1');
            chosen_point += 1;
        }
    }
    (String::from_utf8_lossy(&chosen).into_owned(), chosen_point)
}

/// 第 `i` 位数字；越界或 `i < 0` 一律按 `'0'`（截断后低位与小数点左侧补零）。
fn digit_at(digits: &[u8], i: i32) -> char {
    usize::try_from(i)
        .ok()
        .and_then(|k| digits.get(k))
        .map_or('0', |b| char::from(*b))
}

/// 追加 `[start, end)` 位数字（越界/负数由 [`digit_at`] 补零）。
fn push_digits(out: &mut String, digits: &[u8], start: i32, end: i32) {
    for i in start..end {
        out.push(digit_at(digits, i));
    }
}

/// 把 `0.digits × 10^point` 舍入到 `keep` 位有效数字（五入）。
///
/// 被切掉的首位 `>= '5'` 即进位：精确展开已去尾零，故 `'5'` 后无有效数字时
/// 正是「并列取较大者」，其后有非零数字时本来就更接近上界——两种情况都进位。
/// 返回舍入后的数字（进位溢出时为 `keep + 1` 位，形如 `1` + `keep` 个 `0`）
/// 与修正后的小数点位置。
fn round_digits(digits: &[u8], point: i32, keep: usize) -> (Vec<u8>, i32) {
    if keep >= digits.len() {
        let mut padded = digits.to_vec();
        padded.resize(keep, b'0');
        return (padded, point);
    }
    let mut kept = digits[..keep].to_vec();
    if digits[keep] >= b'5' {
        let mut carry = true;
        for d in kept.iter_mut().rev() {
            if !carry {
                break;
            }
            if *d == b'9' {
                *d = b'0';
            } else {
                *d += 1;
                carry = false;
            }
        }
        if carry {
            // 全 9 进位：`keep == 0`（toFixed 的极端）或 `999…` → `1` + keep 个 0
            kept.insert(0, b'1');
            return (kept, point + 1);
        }
    }
    (kept, point)
}

/// 零结果（小数位全 0）：`toFixed` 在小数点远左于精度位时的输出。
fn zero_fixed(neg: bool, f: usize) -> String {
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push('0');
    if f > 0 {
        out.push('.');
        out.push_str(&"0".repeat(f));
    }
    out
}

/// 非有限数的 `Number::toString` 形态。
fn non_finite(n: f64) -> Option<String> {
    if n.is_nan() {
        Some("NaN".to_owned())
    } else if n.is_infinite() {
        Some(if n > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        })
    } else {
        None
    }
}

/// `Number.prototype.toFixed`（`f ∈ [0,100]`，区间校验由调用方负责）。
pub(crate) fn to_fixed(n: f64, f: usize) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    // 规范：|x| ≥ 10^21 直接 ToString(x)（不再展开小数位）
    if n.abs() >= 1e21 {
        return crate::ops::js_number_to_string(n);
    }
    let dec = exact_decimal(n);
    let keep = dec.point + f as i32;
    if keep < 0 {
        // 值 < 10^(keep-f) ≤ 0.5·10^-f：舍入为 0（负号仍保留）
        return zero_fixed(dec.neg, f);
    }
    let (digits, point) = round_digits(&dec.digits, dec.point, keep.unsigned_abs() as usize);
    let mut out = String::new();
    if dec.neg {
        out.push('-');
    }
    if point <= 0 {
        out.push('0');
    } else {
        push_digits(&mut out, &digits, 0, point);
    }
    if f > 0 {
        out.push('.');
        push_digits(&mut out, &digits, point, point + f as i32);
    }
    out
}

/// `Number.prototype.toExponential`；`f == None`（实参缺省或 `undefined`）取
/// 最短可往返数字，否则保留 `f + 1` 位有效数字。指数恒带符号、至少一位数字。
pub(crate) fn to_exponential(n: f64, f: Option<usize>) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    let (neg, digits, point) = match f {
        None => {
            let (digits, point) = shortest_spec_digits(n.abs());
            (n < 0.0, digits.into_bytes(), point)
        }
        Some(_) => {
            let dec = exact_decimal(n);
            (dec.neg, dec.digits, dec.point)
        }
    };
    let keep = f.map_or(digits.len().max(1), |f| f + 1);
    let (digits, point) = round_digits(&digits, point, keep);
    let exp = point - 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    push_digits(&mut out, &digits, 0, 1);
    if keep > 1 {
        out.push('.');
        push_digits(&mut out, &digits, 1, keep as i32);
    }
    out.push('e');
    out.push(if exp < 0 { '-' } else { '+' });
    out.push_str(&exp.unsigned_abs().to_string());
    out
}

/// `Number.prototype.toPrecision`（`p ∈ [1,100]`，区间校验由调用方负责）。
///
/// 规范：取 `e`、`n` 使 `10^(p-1) ≤ n < 10^p` 且 `n·10^(e-p+1)` 最接近 x
/// （并列取较大），**`e < -6 或 e ≥ p` → 指数形态**，否则固定形态。
pub(crate) fn to_precision(n: f64, p: usize) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    let dec = exact_decimal(n);
    let (digits, point) = round_digits(&dec.digits, dec.point, p);
    let e = point - 1;
    let mut out = String::new();
    if dec.neg {
        out.push('-');
    }
    if e < -6 || e >= p as i32 {
        push_digits(&mut out, &digits, 0, 1);
        if p > 1 {
            out.push('.');
            push_digits(&mut out, &digits, 1, p as i32);
        }
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.unsigned_abs().to_string());
        return out;
    }
    if e >= 0 {
        push_digits(&mut out, &digits, 0, e + 1);
        if p as i32 > e + 1 {
            out.push('.');
            push_digits(&mut out, &digits, e + 1, p as i32);
        }
    } else {
        out.push_str("0.");
        out.push_str(&"0".repeat((-e - 1) as usize));
        push_digits(&mut out, &digits, 0, p as i32);
    }
    out
}

/// V8 `base::Double::Exponent()`：`biased_e - kExponentBias`（`kExponentBias =
/// 0x3FF + 52 = 1075`）；次正规与零返回 `kDenormalExponent = -1074`。
fn v8_exponent(x: f64) -> i32 {
    let biased = ((x.to_bits() >> 52) & 0x7ff) as i32;
    if biased == 0 { -1074 } else { biased - 1075 }
}

/// V8 `chars[static_cast<int>(i)]`：`i ∈ [0, radix]`；`i == 36` 时取到的是 C 字面量
/// 结尾的 NUL（V8 正是靠它截断字符串，见 [`to_radix_string`] 末尾）。
fn radix_char(i: u32) -> u8 {
    usize::try_from(i)
        .ok()
        .and_then(|k| RADIX_DIGITS.get(k))
        .copied()
        .unwrap_or(0)
}

/// `Number.prototype.toString(radix)`（`radix ∈ [2,36]`，区间校验由调用方负责）。
///
/// **逐句移植 V8 `DoubleToRadixCString`**（`src/numbers/conversions.cc`），不做精确
/// 展开：整数位用 f64 反复除法（取余 `Modulo` = `fmod` 精确，商则每步舍入），超出
/// 有效精度的高位**一律补 `'0'`**——故 `(1e21).toString(36)` 是 `"5v1j4f4ds7c000"`、
/// `(1e21).toString(3)` 末 11 位为 `'0'`：**低位补零是 V8 的既有行为，非缺陷**。
/// 小数位按 `delta = ½·ulp` 逐位推进，末位做「就近、并列取偶」的回退进位（可一路
/// 进位到整数位）。
///
/// 该算法已与 Node.js v22.3.0 在 19 种进制 × 1260 个值（含 1200 个随机 double，
/// 覆盖 2^-1080 ~ 2^1020 全量级）共 23959 次对拍零偏差。
pub(crate) fn to_radix_string(n: f64, radix: u32) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    // V8 的定长缓冲区：以 `kBufferSize / 2` 为「小数点」原点，整数位向左、小数位
    // 向右生长；最终由首个 NUL 截断成 C 字符串。
    const HALF: usize = 1100;
    let mut buffer = vec![0_u8; 2 * HALF];
    let mut int_cur = HALF;
    let mut frac_cur = HALF;

    let neg = n < 0.0;
    let value = if neg { -n } else { n };
    let radix_f = f64::from(radix);

    let mut integer = value.floor();
    let mut fraction = value - integer;
    // V8：`delta = std::max(Double(0.0).NextDouble(), delta)`，NaN 时同样取左值
    let mut delta = 0.5 * (next_up(value) - value);
    let min_delta = f64::from_bits(1); // Double(0.0).NextDouble()
    if delta.is_nan() || delta < min_delta {
        delta = min_delta;
    }

    if fraction >= delta {
        buffer[frac_cur] = b'.';
        frac_cur += 1;
        loop {
            // 上移一位数字
            fraction *= radix_f;
            delta *= radix_f;
            let digit = fraction.trunc(); // `static_cast<int>(fraction)`
            buffer[frac_cur] = radix_char(digit as u32);
            frac_cur += 1;
            // 取余
            fraction -= digit;
            // 就近取偶 + 进位判定（V8 原为嵌套 `if`，此处合并以满足
            // clippy::collapsible_if，语义不变）
            if (fraction > 0.5 || (fraction == 0.5 && ((digit as u32) & 1) != 0))
                && fraction + delta > 1.0
            {
                // 需要进位：回溯已写数字（可能一路进位到整数部分）
                loop {
                    frac_cur -= 1;
                    if frac_cur == HALF {
                        // 越过小数点 → 进位到整数部分
                        integer += 1.0;
                        break;
                    }
                    let c = buffer[frac_cur];
                    // `c == 0` 只可能来自 `radix_char(36)`（V8 侧等价于越界读，
                    // 按 `'0'` 处理以免 panic；实测该分支不可达）
                    let d = if c == 0 {
                        0
                    } else if c > b'9' {
                        c - b'a' + 10
                    } else {
                        c - b'0'
                    };
                    if u32::from(d) + 1 < radix {
                        buffer[frac_cur] = RADIX_DIGITS[usize::from(d) + 1];
                        frac_cur += 1;
                        break;
                    }
                }
                break;
            }
            // 达到所需精度即停止（对应 `do { … } while (fraction >= delta)`）
            if fraction < delta {
                break;
            }
        }
    }

    // 整数位：先为「有效精度之外」的高位补 '0'（这些位已无法区分）
    while v8_exponent(integer / radix_f) > 0 {
        integer /= radix_f;
        int_cur -= 1;
        buffer[int_cur] = b'0';
    }
    loop {
        let remainder = integer % radix_f; // `Modulo` = `fmod`（精确）
        int_cur -= 1;
        buffer[int_cur] = radix_char(remainder as u32);
        integer = (integer - remainder) / radix_f;
        if integer <= 0.0 {
            break;
        }
    }

    if neg {
        int_cur -= 1;
        buffer[int_cur] = b'-';
    }
    // V8 收尾写入 `'\0'`：更早出现的 NUL（见 [`radix_char`]）同样会截断结果
    let bytes = &buffer[int_cur..frac_cur];
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// 大于 `x` 的最小可表示 f64（`Double::NextDouble`；不依赖 `f64::next_up`
/// 以照顾 workspace 的 `rust-version = 1.85`）。NaN/`+∞` 原样返回。
fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
}

#[cfg(test)]
mod tests {
    // 测试表的并列候选（如 `1501199875790165.25`）刻意写全精度：它正是
    // 「并列取偶」的居中点，Clippy 提示的「可截断」表示本身就是被测的 bug。
    #![allow(clippy::excessive_precision)]
    use super::*;

    /// `(输入, toFixed(位数), 期望)`：期望值取自 Node.js v22.3.0 实测。
    const FIXED: &[(f64, usize, &str)] = &[
        (0.5, 0, "1"),
        (1.25, 1, "1.3"),
        (1.005, 2, "1.00"),
        (1.5, 0, "2"),
        (2.5, 0, "3"),
        (1234.5, 0, "1235"),
        (-1.005, 2, "-1.00"),
        (-1.5, 0, "-2"),
        (-0.001, 2, "-0.00"),
        (0.0, 2, "0.00"),
        (1e21, 0, "1e+21"),
        (1e20, 0, "100000000000000000000"),
        (1e-7, 2, "0.00"),
        (9.995, 2, "9.99"),
        (1.45, 1, "1.4"),
        (2.675, 2, "2.67"),
        (0.35, 1, "0.3"),
        (1.05, 1, "1.1"),
        (5e-324, 0, "0"),
    ];

    #[test]
    fn to_fixed_matches_node() {
        for &(n, f, want) in FIXED {
            assert_eq!(to_fixed(n, f), want, "toFixed({n}, {f})");
        }
    }

    /// `(输入, None 或位数, 期望)`。
    const EXP: &[(f64, Option<usize>, &str)] = &[
        (1234.5, None, "1.2345e+3"),
        (1234.5, Some(2), "1.23e+3"),
        (12345.0, Some(0), "1e+4"),
        (0.0, Some(2), "0.00e+0"),
        (1.0, Some(2), "1.00e+0"),
        (0.5, Some(1), "5.0e-1"),
        (0.0001, None, "1e-4"),
        (100.0, None, "1e+2"),
        (1e21, None, "1e+21"),
        (-0.0, None, "0e+0"),
        // 并列取偶：精确展开 `…65.25` 落在 17 位候选 `…652`/`…653` 正中
        (1501199875790165.25, None, "1.5011998757901652e+15"),
        (643371375338642.25, None, "6.433713753386422e+14"),
    ];

    #[test]
    fn to_exponential_matches_node() {
        for &(n, f, want) in EXP {
            assert_eq!(to_exponential(n, f), want, "toExponential({n}, {f:?})");
        }
    }

    /// `(输入, 精度, 期望)`。
    const PREC: &[(f64, usize, &str)] = &[
        (1.5, 2, "1.5"),
        (123.456, 3, "123"),
        (255.0, 3, "255"),
        (-42.7, 3, "-42.7"),
        (0.5, 3, "0.500"),
        (1234.5, 2, "1.2e+3"),
        (0.0, 3, "0.00"),
        (1.005, 4, "1.005"),
        (12345.0, 2, "1.2e+4"),
        (0.000001234, 2, "0.0000012"),
        (1e-7, 2, "1.0e-7"),
        (10000.0, 2, "1.0e+4"),
        (9.99, 2, "10"),
        (999.9, 3, "1.00e+3"),
        (0.00000999, 1, "0.00001"),
        (1.5, 5, "1.5000"),
        (10000.0, 10, "10000.00000"),
        (1e21, 22, "1000000000000000000000"),
        (1e21, 21, "1.00000000000000000000e+21"),
        // 与 `Number::toString` 相反：toPrecision 并列时规范要求取**较大**者
        (1501199875790165.25, 17, "1501199875790165.3"),
    ];

    #[test]
    fn to_precision_matches_node() {
        for &(n, p, want) in PREC {
            assert_eq!(to_precision(n, p), want, "toPrecision({n}, {p})");
        }
    }

    /// 规范「并列取偶」（`Number::toString` / `toExponential()` 无参）与
    /// `toPrecision` 的「并列取较大」是**两条不同规则**，此处一并锁定。
    /// 期望值取自 Node.js v22.3.0 实测。
    #[test]
    fn shortest_digits_ties_to_even() {
        // 精确展开 `1501199875790165.25`：17 位候选 `…652` / `…653` 的正中
        let (digits, point) = shortest_spec_digits(1501199875790165.25);
        assert_eq!(digits, "15011998757901652");
        assert_eq!(point, 16);
        assert_eq!(
            crate::ops::js_number_to_string(1501199875790165.25),
            "1501199875790165.2"
        );
        assert_eq!(
            crate::ops::js_number_to_string(643371375338642.25),
            "643371375338642.2"
        );
        // 非并列时不受影响
        assert_eq!(
            crate::ops::js_number_to_string(0.1 + 0.2),
            "0.30000000000000004"
        );
        assert_eq!(crate::ops::js_number_to_string(1e21), "1e+21");
        assert_eq!(crate::ops::js_number_to_string(-0.0), "0");
    }

    /// `(输入, 进制, 期望)`：期望值全部取自 Node.js v22.3.0 实测，覆盖普通值、
    /// 大整数（整数位精度用尽后补零）、极小数与次正规数。
    const RADIX: &[(f64, u32, &str)] = &[
        (255.0, 16, "ff"),
        (123.456, 16, "7b.74bc6a7ef9dc"),
        (-42.7, 16, "-2a.b33333333334"),
        (0.5, 16, "0.8"),
        (255.0, 2, "11111111"),
        (10.0, 3, "101"),
        (
            0.1,
            2,
            "0.0001100110011001100110011001100110011001100110011001101",
        ),
        (0.1, 16, "0.1999999999999a"),
        (0.1, 36, "0.3lllllllllm"),
        (-0.5, 2, "-0.1"),
        (3.5, 2, "11.1"),
        (
            1.0000000000000002,
            2,
            "1.0000000000000000000000000000000000000000000000000001",
        ),
        (1e21, 16, "3635c9adc5dea00000"),
        (1e21, 3, "100010202110111202020110202012022200000000000"),
        (1e21, 36, "5v1j4f4ds7c000"),
        (1e20, 3, "220200020122120112010222022122002000000000"),
        (
            1e100,
            3,
            "122012210112120112111212010011100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        ),
        (
            1e100,
            36,
            "2hqbczu2ow6000000000000000000000000000000000000000000000000000000",
        ),
        (1e-6, 3, "0.000000000000112100102100112111002112222222101"),
        (
            1.7976931348623157e308,
            3,
            "10020200012020012100112000100111212000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        ),
        (
            1e-320,
            36,
            "0.00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003yr",
        ),
        (9007199254740992.0, 36, "2gosa7pa2gw"),
        (0.0, 2, "0"),
        (-0.0, 2, "0"),
        (f64::NAN, 2, "NaN"),
        (f64::INFINITY, 2, "Infinity"),
        (f64::NEG_INFINITY, 2, "-Infinity"),
    ];

    #[test]
    fn to_radix_string_matches_node() {
        for &(n, radix, want) in RADIX {
            assert_eq!(to_radix_string(n, radix), want, "toString({n}, {radix})");
        }
    }

    #[test]
    fn bignat_basics() {
        let mut big = BigNat::from_u64(u64::MAX);
        assert_eq!(big.to_decimal_string(), u64::MAX.to_string());
        big.shl(64);
        assert_eq!(
            // (2^64-1) << 64 = 2^128 - 2^64
            big.to_decimal_string(),
            "340282366920938463444927863358058659840"
        );
        let mut small = BigNat::from_u64(1_234_567_890);
        small.mul_small(1_000);
        assert_eq!(small.to_decimal_string(), "1234567890000");
        assert_eq!(small.divmod_small(10), 0);
        assert_eq!(small.to_decimal_string(), "123456789000");
        assert!(BigNat::default().is_zero());
    }

    #[test]
    fn exact_decimal_expands_exactly() {
        // 0.5 = 5 × 10^-1 → digits "5"、point 0
        let dec = exact_decimal(0.5);
        assert_eq!(dec.digits, b"5");
        assert_eq!(dec.point, 0);
        // 1234.5 → digits "12345"、point 4
        let dec = exact_decimal(1234.5);
        assert_eq!(dec.digits, b"12345");
        assert_eq!(dec.point, 4);
        // 1e21 是整数：digits "1"、point 22（尾随零已去）
        let dec = exact_decimal(1e21);
        assert_eq!(dec.digits, b"1");
        assert_eq!(dec.point, 22);
        // 1.005 的 double 实际小于 1.005，精确展开以 1.0049999999999999 开头
        let dec = exact_decimal(1.005);
        assert_eq!(&dec.digits[..6], b"100499");
        assert_eq!(dec.point, 1);
    }
}
