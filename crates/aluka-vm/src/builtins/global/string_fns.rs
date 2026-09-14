//! String 构造器静态方法：fromCharCode / fromCodePoint。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use crate::value::ValueCase;

/// `String.raw(template, ...substitutions)`：按模板对象的 **raw** 字面量
/// 拼接（`raw` 属性与 cooked 同长；替换位按序插入并在末尾补足剩余段）。
pub(crate) fn string_raw(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(tmpl) = args.first().copied() else {
        return Err(vm.type_error("String.raw requires a template object"));
    };
    let Some(tr) = tmpl.as_object() else {
        return Err(vm.type_error("String.raw requires a template object"));
    };
    let raw = vm.get_property(Value::Object(tr), "raw")?;
    let Some(rr) = raw.as_object() else {
        return Err(vm.type_error("String.raw template object has no raw array"));
    };
    let len = match vm.get_property(Value::Object(rr), "length")?.case() {
        ValueCase::Number(n) => n.max(0.0) as usize,
        _ => 0,
    };
    let mut out = String::new();
    for i in 0..len {
        let seg = vm.get_property(Value::Object(rr), &i.to_string())?;
        out.push_str(&vm.format_value(seg));
        if i + 1 < len
            && let Some(sub) = args.get(i + 1)
        {
            out.push_str(&vm.format_value(*sub));
        }
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

pub(crate) fn string_from_char_code(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args {
        // ToNumber 全语义：字符串参数按 JS 数字字面量解析（十六进制
        // "0x41" 形态——`String.fromCharCode("0x41")` → 'A'；此前字符串
        // 恒 NaN → 全部产出 U+FFFD）
        let code = vm.to_number_value(*arg) as u32;
        if code < 0x10000 {
            s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
        } else if code < 0x110000 {
            let hi = 0xD800 + ((code - 0x10000) >> 10);
            let lo = 0xDC00 + ((code - 0x10000) & 0x3FF);
            s.push(char::from_u32(hi).unwrap_or('\u{FFFD}'));
            s.push(char::from_u32(lo).unwrap_or('\u{FFFD}'));
        } else {
            s.push('\u{FFFD}');
        }
    }
    Ok(Value::Object(vm.alloc_string(s)))
}

pub(crate) fn string_from_code_point(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args {
        let code = vm.to_number_value(*arg) as u32;
        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
    }
    Ok(Value::Object(vm.alloc_string(s)))
}
