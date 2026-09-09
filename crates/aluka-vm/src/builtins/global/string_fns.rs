//! String 构造器静态方法：fromCharCode / fromCodePoint。

use crate::interpreter::{Vm, VmError};
use crate::ops::to_number;
use crate::value::Value;

pub(crate) fn string_from_char_code(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args {
        let code = to_number(*arg) as u32;
        if code < 0x10000 { s.push(char::from_u32(code).unwrap_or('\u{FFFD}')); }
        else if code < 0x110000 {
            let hi = 0xD800 + ((code - 0x10000) >> 10);
            let lo = 0xDC00 + ((code - 0x10000) & 0x3FF);
            s.push(char::from_u32(hi).unwrap_or('\u{FFFD}'));
            s.push(char::from_u32(lo).unwrap_or('\u{FFFD}'));
        } else { s.push('\u{FFFD}'); }
    }
    Ok(Value::Object(vm.alloc_string(s)))
}

pub(crate) fn string_from_code_point(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args { let code = to_number(*arg) as u32; s.push(char::from_u32(code).unwrap_or('\u{FFFD}')); }
    Ok(Value::Object(vm.alloc_string(s)))
}