//! URI 编解码函数。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;

fn is_uri_unreserved(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}

fn encode_component(vm: &mut Vm, args: &[Value], extra_safe: &[u8]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let mut out = String::new();
    for byte in text.as_bytes() {
        if is_uri_unreserved(*byte) || extra_safe.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

fn decode_component_impl(
    vm: &mut Vm,
    args: &[Value],
    plus_as_space: bool,
) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    // 非十六进制转义（`%zz` / `%2x`）：规范 URIError——
                    // 此前按字面透传（Node 22 实测恒抛）
                    Err(_) => return Err(vm.typed_error("URIError", "URI malformed")),
                }
            }
            // 截断的转义（`%` 悬尾 / `%4`）：规范 URIError
            b'%' => return Err(vm.typed_error("URIError", "URI malformed")),
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(Value::Object(vm.alloc_string(
        match String::from_utf8(out) {
            Ok(s) => s,
            Err(_) => return Err(vm.typed_error("URIError", "URI malformed")),
        },
    )))
}

pub(crate) fn encode_uri_component(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    encode_component(vm, args, &[])
}
pub(crate) fn decode_uri_component(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    decode_component_impl(vm, args, false)
}
pub(crate) fn encode_uri(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    encode_component(vm, args, b";/?:@&=+$,#")
}
pub(crate) fn decode_uri(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    decode_component_impl(vm, args, false)
}
