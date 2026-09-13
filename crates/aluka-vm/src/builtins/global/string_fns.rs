//! String 构造器静态方法：fromCharCode / fromCodePoint。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;

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
