//! `punycode` 内置模块（Phase 8）：废弃（DEP0040）的 Punycode 编解码。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodeutil/punycode.go`，punycode.js 2.1.0 语义，
//! RFC 3492）：
//! - `encode(input)` / `decode(input)`：Unicode ↔ Punycode（纯 ASCII）；
//! - `toASCII(domain)` / `toUnicode(domain)`：域名/邮箱标签变换（`xn--`）；
//! - `ucs2.decode/encode`：码点数组 ↔ 字符串；
//! - `version`：`"2.1.0"`。
//!
//! 错误语义对齐 Go：非法/溢出输入抛消息一致的异常（Go 侧经
//! `engine.ErrRangeError` 呈现为 RangeError；Rust 侧抛字符串异常值，
//! 消息逐字一致）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("punycode")` / `require("node:punycode")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "punycode",
    build,
};

// --- RFC 3492 常量（与 punycode.js 2.1.0 / Go 实现一致） ---

const PC_MAX_INT: i64 = 2147483647;
const PC_BASE: i64 = 36;
const PC_TMIN: i64 = 1;
const PC_TMAX: i64 = 26;
const PC_SKEW: i64 = 38;
const PC_DAMP: i64 = 700;
const PC_INITIAL_BIAS: i64 = 72;
const PC_INITIAL_N: i64 = 128;
const PC_DELIMITER: char = '-';
const PC_BASE_MINUS_TMIN: i64 = PC_BASE - PC_TMIN;

/// `basicToDigit`：严格范围检查（非字母数字返回 base，与 npm punycode
/// 2.1.0 的宽松判断不同）。
fn basic_to_digit(code_point: u8) -> i64 {
    if (0x30..0x3A).contains(&code_point) {
        return 26 + i64::from(code_point - 0x30);
    }
    if (0x41..0x5B).contains(&code_point) {
        return i64::from(code_point - 0x41);
    }
    if (0x61..0x7B).contains(&code_point) {
        return i64::from(code_point - 0x61);
    }
    PC_BASE
}

/// `digitToBasic`：0..25 → a..z / A..Z；26..35 → 0..9。
fn digit_to_basic(digit: i64, flag: i64) -> char {
    let cp = if digit < 26 {
        digit + 97 - (flag << 5)
    } else {
        digit - 26 + 48 - (flag << 5)
    };
    char::from_u32(cp as u32).unwrap_or('?')
}

/// `adapt`（RFC 3492 §3.4）。
fn adapt(mut delta: i64, num_points: i64, first_time: bool) -> i64 {
    if first_time {
        delta /= PC_DAMP;
    } else {
        delta >>= 1;
    }
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((PC_BASE_MINUS_TMIN * PC_TMAX) >> 1) {
        delta /= PC_BASE_MINUS_TMIN;
        k += PC_BASE;
    }
    k + (PC_BASE_MINUS_TMIN + 1) * delta / (delta + PC_SKEW)
}

/// `ucs2decode`：字符串 → 码点数组（按 Unicode 码点迭代）。
fn ucs2_decode(input: &str) -> Vec<i64> {
    input.chars().map(|c| i64::from(u32::from(c))).collect()
}

/// `ucs2encode`：码点数组 → 字符串（`String.fromCodePoint` 语义）。
fn ucs2_encode(code_points: &[i64]) -> String {
    code_points
        .iter()
        .filter_map(|&cp| char::from_u32(cp as u32))
        .collect()
}

/// `decode`：Punycode → Unicode。按**字节**扫描输入（对齐 Go `len`/下标语义）。
fn punycode_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let input_length = bytes.len() as i64;
    let mut output: Vec<i64> = Vec::new();
    let mut i = 0;
    let mut n = PC_INITIAL_N;
    let mut bias = PC_INITIAL_BIAS;

    let mut basic = input.rfind(PC_DELIMITER).map_or(-1, |p| p as i64);
    if basic < 0 {
        basic = 0;
    }
    for j in 0..basic {
        let b = bytes[j as usize];
        if b >= 0x80 {
            return Err("Illegal input >= 0x80 (not a basic code point)".to_owned());
        }
        output.push(i64::from(b));
    }

    let mut index = if basic > 0 { basic + 1 } else { 0 };
    while index < input_length {
        let oldi = i;
        let mut w = 1;
        let mut k = PC_BASE;
        loop {
            if index >= input_length {
                return Err("Invalid input".to_owned());
            }
            let digit = basic_to_digit(bytes[index as usize]);
            index += 1;
            if digit >= PC_BASE {
                return Err("Invalid input".to_owned());
            }
            if digit > (PC_MAX_INT - i) / w {
                return Err("Overflow: input needs wider integers to process".to_owned());
            }
            i += digit * w;
            let t = if k <= bias {
                PC_TMIN
            } else if k >= bias + PC_TMAX {
                PC_TMAX
            } else {
                k - bias
            };
            if digit < t {
                break;
            }
            let base_minus_t = PC_BASE - t;
            if w > PC_MAX_INT / base_minus_t {
                return Err("Overflow: input needs wider integers to process".to_owned());
            }
            w *= base_minus_t;
            k += PC_BASE;
        }
        let out = output.len() as i64 + 1;
        bias = adapt(i - oldi, out, oldi == 0);
        if i / out > PC_MAX_INT - n {
            return Err("Overflow: input needs wider integers to process".to_owned());
        }
        n += i / out;
        i %= out;
        output.insert(i as usize, n);
        i += 1;
    }
    Ok(ucs2_encode(&output))
}

/// `encode`：Unicode → Punycode。
fn punycode_encode(input: &str) -> Result<String, String> {
    let code_points = ucs2_decode(input);
    let input_length = code_points.len() as i64;

    let mut n = PC_INITIAL_N;
    let mut delta = 0;
    let mut bias = PC_INITIAL_BIAS;
    let mut output: Vec<char> = Vec::new();

    for &cp in &code_points {
        if cp < 0x80 {
            output.push(char::from_u32(cp as u32).unwrap_or('?'));
        }
    }
    let basic_length = output.len() as i64;
    let mut handled_cp_count = basic_length;
    if basic_length > 0 {
        output.push(PC_DELIMITER);
    }

    while handled_cp_count < input_length {
        let mut m = PC_MAX_INT;
        for &cp in &code_points {
            if cp >= n && cp < m {
                m = cp;
            }
        }
        let handled_cp_count_plus_one = handled_cp_count + 1;
        if m - n > (PC_MAX_INT - delta) / handled_cp_count_plus_one {
            return Err("Overflow: input needs wider integers to process".to_owned());
        }
        delta += (m - n) * handled_cp_count_plus_one;
        n = m;

        for &cp in &code_points {
            if cp < n {
                delta += 1;
                if delta > PC_MAX_INT {
                    return Err("Overflow: input needs wider integers to process".to_owned());
                }
            }
            if cp == n {
                let mut q = delta;
                let mut k = PC_BASE;
                loop {
                    let t = if k <= bias {
                        PC_TMIN
                    } else if k >= bias + PC_TMAX {
                        PC_TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    let q_minus_t = q - t;
                    let base_minus_t = PC_BASE - t;
                    output.push(digit_to_basic(t + q_minus_t % base_minus_t, 0));
                    q = q_minus_t / base_minus_t;
                    k += PC_BASE;
                }
                output.push(digit_to_basic(q, 0));
                bias = adapt(
                    delta,
                    handled_cp_count_plus_one,
                    handled_cp_count == basic_length,
                );
                delta = 0;
                handled_cp_count += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Ok(output.into_iter().collect())
}

/// `mapDomain`：邮箱 local part 不动；Unicode 句点统一为 `.` 后逐标签应用 fn。
fn map_domain<F>(domain: &str, f: F) -> String
where
    F: FnMut(&str) -> String,
{
    let mut result = String::new();
    let mut rest = domain;
    if let Some(at) = domain.find('@') {
        result.push_str(&domain[..=at]);
        rest = &domain[at + 1..];
    }
    let normalized = rest.replace(['\u{3002}', '\u{FF0E}', '\u{FF61}'], ".");
    let labels: Vec<String> = normalized.split('.').map(f).collect();
    result.push_str(&labels.join("."));
    result
}

/// 判断字符串是否含非 ASCII 字符（>= 0x80 才需要 Punycode 编码）。
fn has_non_ascii(s: &str) -> bool {
    s.bytes().any(|b| b >= 0x80)
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    let version = Value::Object(vm.alloc_string("2.1.0".to_owned()));
    set_module_prop(vm, obj, "version", version)?;

    // ucs2 子对象：`_builtinNs` 实例分派（引擎 CALL_METHOD 无属性回退，
    // 二级属性调用按 `{ns}.{method}` 命中）。
    let ucs2 = vm.alloc_ordinary();
    let ns_val = Value::Object(vm.alloc_string("punycode:ucs2".to_owned()));
    let _ = vm.set_property(Value::Object(ucs2), "_builtinNs", ns_val);
    let ucs2_decode_fn = vm.alloc_native_fn("punycode:ucs2.decode");
    let ucs2_encode_fn = vm.alloc_native_fn("punycode:ucs2.encode");
    set_module_prop(vm, ucs2, "decode", Value::Object(ucs2_decode_fn))?;
    set_module_prop(vm, ucs2, "encode", Value::Object(ucs2_encode_fn))?;
    set_module_prop(vm, obj, "ucs2", Value::Object(ucs2))?;

    for (prop, name) in [
        ("decode", "punycode.decode"),
        ("encode", "punycode.encode"),
        ("toUnicode", "punycode.toUnicode"),
        ("toASCII", "punycode.toASCII"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, prop, Value::Object(fn_ref))?;
    }

    register_handler(registry, "punycode", "decode", decode);
    register_handler(registry, "punycode", "encode", encode);
    register_handler(registry, "punycode", "toUnicode", to_unicode);
    register_handler(registry, "punycode", "toASCII", to_ascii);
    register_handler(registry, "punycode:ucs2", "decode", ucs2_decode_handler);
    register_handler(registry, "punycode:ucs2", "encode", ucs2_encode_handler);

    Ok(obj)
}

/// 读字符串参数（缺省空串，对齐 Go `nodebase.StrArg`）。
fn str_arg(vm: &mut Vm, args: &[Value], idx: usize) -> String {
    args.get(idx)
        .map(|v| vm.format_value(*v))
        .unwrap_or_default()
}

/// 以消息字符串构造异常（Go 侧为 RangeError 语义：异常为带 message
/// 属性的错误对象；name 为引擎通用的 "Error"——已知呈现差异）。
fn thrown_msg(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(msg)))
}

/// `punycode.decode(input)`：Punycode → Unicode。
fn decode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = str_arg(vm, args, 0);
    match punycode_decode(&input) {
        Ok(out) => Ok(Value::Object(vm.alloc_string(out))),
        Err(msg) => Err(thrown_msg(vm, &msg)),
    }
}

/// `punycode.encode(input)`：Unicode → Punycode。
fn encode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = str_arg(vm, args, 0);
    match punycode_encode(&input) {
        Ok(out) => Ok(Value::Object(vm.alloc_string(out))),
        Err(msg) => Err(thrown_msg(vm, &msg)),
    }
}

/// `punycode.toUnicode(domain)`：xn-- 标签解码（解码失败保持原标签）。
fn to_unicode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = str_arg(vm, args, 0);
    let out = map_domain(&input, |label| {
        if let Some(rest) = label.strip_prefix("xn--") {
            if let Ok(dec) = punycode_decode(&rest.to_lowercase()) {
                return dec;
            }
        }
        label.to_owned()
    });
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `punycode.toASCII(domain)`：非 ASCII 标签编码为 xn--（编码失败保持原标签）。
fn to_ascii(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = str_arg(vm, args, 0);
    let out = map_domain(&input, |label| {
        if has_non_ascii(label) {
            if let Ok(enc) = punycode_encode(label) {
                return format!("xn--{enc}");
            }
        }
        label.to_owned()
    });
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `punycode.ucs2.decode(str)`：字符串 → 码点数组。
fn ucs2_decode_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = str_arg(vm, args, 0);
    let cps = ucs2_decode(&input);
    let elems: Vec<Value> = cps.into_iter().map(|cp| Value::Number(cp as f64)).collect();
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `punycode.ucs2.encode(arr)`：码点数组 → 字符串。
fn ucs2_encode_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut cps: Vec<i64> = Vec::new();
    if let Some(r) = args.first().as_object {
        if let Some(HeapObject::Array { elements, .. }) = vm.heap.get(r.index()) {
            for e in elements {
                if let Some(n) = e.as_number {
                    cps.push(*n as i64);
                }
            }
        }
    }
    Ok(Value::Object(vm.alloc_string(ucs2_encode(&cps))))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = decode;
        let _: crate::builtins::BuiltinHandler = encode;
        let _: crate::builtins::BuiltinHandler = to_unicode;
        let _: crate::builtins::BuiltinHandler = to_ascii;
    }

    /// RFC 3492 与 Node.js 22 LTS 标准实测样例。
    #[test]
    fn punycode_rfc3492_cases() {
        for (input, expected) in [
            ("", ""),
            ("Hello", "Hello-"),
            ("abc", "abc-"),
            ("bücher", "bcher-kva"),
            ("münchen", "mnchen-3ya"),
            ("中国", "fiqs8s"),
            ("日本語", "wgv71a119e"),
            ("Ω", "exa"),
            ("αβγδ", "mxacde"),
            ("München-Ost", "Mnchen-Ost-9db"),
            ("мойдомен.рф", ".-gtbdoocidcy8b"),
            ("-abc-", "-abc--"),
            ("a-b", "a-b-"),
            ("--", "---"),
        ] {
            assert_eq!(punycode_encode(input).unwrap(), expected, "encode {input}");
            assert_eq!(
                punycode_decode(expected).unwrap(),
                input,
                "decode {expected}"
            );
        }
        // 域名/邮箱变换（Go toASCII/toUnicode 实测值）。
        assert_eq!(
            map_domain("bücher.ch", |l| {
                if has_non_ascii(l) {
                    format!("xn--{}", punycode_encode(l).unwrap())
                } else {
                    l.to_owned()
                }
            }),
            "xn--bcher-kva.ch"
        );
        assert_eq!(
            map_domain("xn--Mnchen-Ost-9db", |l| l
                .strip_prefix("xn--")
                .and_then(|rest| punycode_decode(&rest.to_lowercase()).ok())
                .unwrap_or_else(|| l.to_owned())),
            "münchen-ost"
        );
        // 非法输入。
        assert_eq!(
            punycode_decode("\u{ff}-??").unwrap_err(),
            "Illegal input >= 0x80 (not a basic code point)"
        );
    }
}
