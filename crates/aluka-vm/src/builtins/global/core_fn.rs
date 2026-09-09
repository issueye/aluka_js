//! ECMAScript 核心全局函数：isNaN / isFinite / parseFloat / parseInt。

use crate::interpreter::{Vm, VmError};
use crate::ops::to_number;
use crate::value::Value;

/// 提取首参为数值（JS ToNumber 语义，堆字符串须解析）。
pub(crate) fn arg_number(vm: &mut Vm, args: &[Value]) -> f64 {
    args.first().map(|v| vm.to_number_value(*v)).unwrap_or(f64::NAN)
}

pub(crate) fn global_is_nan(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(arg_number(vm, args).is_nan()))
}

pub(crate) fn global_is_finite(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let n = arg_number(vm, args);
    Ok(Value::Boolean(n.is_finite()))
}

pub(crate) fn global_parse_float(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args.first().map(|v| vm.format_value(*v)).unwrap_or_default();
    let t = text.trim_start();
    let mut end = 0usize;
    let bytes: Vec<char> = t.chars().collect();
    let mut seen_dot = false;
    let mut seen_exp = false;
    let mut prev_digit = false;
    for (i, c) in bytes.iter().enumerate() {
        if c.is_ascii_digit() { prev_digit = true; end = i + 1; continue; }
        if (*c == '+' || *c == '-') && (i == 0 || (seen_exp && !prev_digit)) { end = i + 1; continue; }
        if *c == '.' && !seen_dot && !seen_exp { seen_dot = true; end = i + 1; continue; }
        if (*c == 'e' || *c == 'E') && prev_digit && !seen_exp { seen_exp = true; prev_digit = false; end = i + 1; continue; }
        break;
    }
    let prefix: String = bytes[..end].iter().collect();
    let n = prefix.parse::<f64>().unwrap_or(f64::NAN);
    Ok(Value::Number(n))
}

pub(crate) fn global_parse_int(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args.first().map(|v| vm.format_value(*v)).unwrap_or_default();
    let radix = args.get(1).map(|v| to_number(*v)).unwrap_or(0.0);
    let t = text.trim_start();
    let (t, radix) = if radix == 0.0 || radix.is_nan() {
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { (hex, 16.0) } else { (t, 10.0) }
    } else { (t, radix) };
    let mut end = 0usize;
    let chars: Vec<char> = t.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let is_digit = c.is_ascii_digit() || c.is_ascii_alphabetic();
        if !is_digit { break; }
        let v = c.to_digit(36).unwrap_or(36) as f64;
        if v >= radix { break; }
        end = i + 1;
    }
    if end == 0 { return Ok(Value::Number(f64::NAN)); }
    let prefix: String = chars[..end].iter().collect();
    match i64::from_str_radix(&prefix, radix as u32) {
        Ok(v) => Ok(Value::Number(v as f64)),
        Err(_) => Ok(Value::Number(f64::NAN)),
    }
}