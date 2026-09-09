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
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
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
            Err(_) => {
                let err = vm.alloc_string("URI malformed".to_owned());
                let name = vm.alloc_string("URIError".to_owned());
                let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                return Err(VmError::Thrown(Value::Object(err)));
            }
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
