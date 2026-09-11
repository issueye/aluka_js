//! FormData 全局构造器 + multipart/form-data 编解码。

use crate::builtins::pending_native_name;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

/// FormData 实例方法统一分派。
pub(crate) fn form_data_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = pending_native_name();
    let receiver = crate::builtins::current_receiver();
    let Some(key) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Undefined);
    };
    let mut entries = fd_entries(vm, receiver);
    match name.as_str() {
        "FormData.append" => {
            let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            entries.push((key, value));
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.set" => {
            let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            match entries.iter_mut().find(|(k, _)| k == &key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key, value)),
            }
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.get" => Ok(entries
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, v)| {
                let s = vm.alloc_string(v.clone());
                Value::Object(s)
            })
            .unwrap_or(Value::Null)),
        "FormData.getAll" => {
            let vals: Vec<Value> = entries
                .iter()
                .filter(|(k, _)| k == &key)
                .map(|(_, v)| {
                    let s = vm.alloc_string(v.clone());
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        "FormData.has" => Ok(Value::Boolean(entries.iter().any(|(k, _)| k == &key))),
        "FormData.delete" => {
            entries.retain(|(k, _)| k != &key);
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.entries" => {
            let pairs: Vec<Value> = entries
                .iter()
                .map(|(k, v)| {
                    let ks = vm.alloc_string(k.clone());
                    let vs = vm.alloc_string(v.clone());
                    let pair = vm.alloc_array(vec![Value::Object(ks), Value::Object(vs)]);
                    Value::Object(pair)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(pairs)))
        }
        "FormData.keys" => {
            let mut keys: Vec<String> = Vec::new();
            for (k, _) in &entries {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
            let vals: Vec<Value> = keys
                .into_iter()
                .map(|k| {
                    let s = vm.alloc_string(k);
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        "FormData.values" => {
            let vals: Vec<Value> = entries
                .iter()
                .map(|(_, v)| {
                    let s = vm.alloc_string(v.clone());
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        _ => {
            let Some(cb) = args
                .first()
                .copied()
                .filter(|v| super::event::is_callable(vm, *v))
            else {
                return Ok(Value::Undefined);
            };
            let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
            for (k, v) in entries {
                let vs = vm.alloc_string(v);
                let ks = vm.alloc_string(k);
                let _ = vm.invoke_callable(
                    cb,
                    this_arg,
                    &[Value::Object(vs), Value::Object(ks), receiver],
                );
            }
            Ok(Value::Undefined)
        }
    }
}

// ---- 内部辅助 ----

pub(crate) fn fd_entries(vm: &mut Vm, receiver: Value) -> Vec<(String, String)> {
    let Ok(ValueCase::Object(arr)) = vm.get_property(receiver, "_fdEntries").map(ValueCase::from)
    else {
        return Vec::new();
    };
    let elements: Vec<Value> = match vm.heap.get(arr.0 as usize) {
        Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
        _ => Vec::new(),
    };
    elements
        .iter()
        .filter_map(|e| {
            let ValueCase::Object(_) = e.case() else {
                return None;
            };
            let name = vm
                .get_property(*e, "name")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            let value = vm
                .get_property(*e, "value")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            Some((name, value))
        })
        .collect()
}

fn fd_rewrite(vm: &mut Vm, receiver: Value, entries: Vec<(String, String)>) {
    let vals: Vec<Value> = entries
        .into_iter()
        .map(|(name, value)| {
            let entry = vm.alloc_ordinary();
            let n = vm.alloc_string(name);
            let _ = vm.set_property(Value::Object(entry), "name", Value::Object(n));
            let v = vm.alloc_string(value);
            let _ = vm.set_property(Value::Object(entry), "value", Value::Object(v));
            Value::Object(entry)
        })
        .collect();
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(receiver, "_fdEntries", Value::Object(arr));
}

/// `new FormData()`：创建空表单。
pub(crate) fn form_data_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let fd = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(fd), "_isFormData", Value::Boolean(true));
    let ns = vm.alloc_string("FormData".to_owned());
    let _ = vm.set_property(Value::Object(fd), "_builtinNs", Value::Object(ns));
    let entries = vm.alloc_array(Vec::new());
    let _ = vm.set_property(Value::Object(fd), "_fdEntries", Value::Object(entries));
    for method in [
        "append", "set", "get", "getAll", "has", "delete", "entries", "keys", "values", "forEach",
    ] {
        let f = vm.alloc_native_fn(&format!("FormData.{method}"));
        let _ = vm.set_property(Value::Object(fd), method, Value::Object(f));
    }
    Ok(Value::Object(fd))
}

// ---- multipart/form-data ----

fn generate_boundary() -> String {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5eed_1234_abcd_5678);
    let mut out = String::new();
    for _ in 0..12 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push(char::from_digit((seed >> 33) as u32 % 16, 16).unwrap_or('0'));
    }
    out
}

/// FormData → multipart/form-data 请求体。
pub fn encode_form_data_multipart(vm: &mut Vm, fd: Value) -> Result<(String, String), VmError> {
    let entries = fd_entries(vm, fd);
    let boundary = format!("----AlukaFormBoundary{}", generate_boundary());
    let mut body = String::new();
    for (name, value) in entries.iter() {
        body.push_str("--");
        body.push_str(&boundary);
        body.push_str("\r\nContent-Disposition: form-data; name=\"");
        body.push_str(&name.replace('\\', "\\\\").replace('"', "\\\""));
        body.push_str("\"\r\n\r\n");
        body.push_str(value);
        body.push_str("\r\n");
    }
    if !entries.is_empty() {
        body.push_str("--");
        body.push_str(&boundary);
        body.push_str("--\r\n");
    }
    let ctype = format!("multipart/form-data; boundary={boundary}");
    Ok((body, ctype))
}

/// 解析 multipart/urlencoded 响应体为 FormData。
fn parse_form_data_body(vm: &mut Vm, ctype: &str, body: &str) -> Result<Value, VmError> {
    let fd = form_data_ctor_impl(vm, &[])?;
    if let Some(bi) = ctype.find("boundary=") {
        let boundary = ctype[bi + "boundary=".len()..]
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_owned();
        let delim = format!("--{boundary}");
        for part in body.split(&delim) {
            let part = part.trim_start_matches('\r').trim_start_matches('\n');
            if part.is_empty() || part.starts_with("--") {
                continue;
            }
            let Some(hdr_end) = part.find("\r\n\r\n") else {
                continue;
            };
            let headers = &part[..hdr_end];
            let value = part[hdr_end + 4..]
                .trim_end_matches('\r')
                .trim_end_matches('\n');
            let mut name = String::new();
            for line in headers.split("\r\n") {
                if let Some(rest) = line.strip_prefix("Content-Disposition:").map(str::trim) {
                    for seg in rest.split(';') {
                        let seg = seg.trim();
                        if let Some(n) = seg.strip_prefix("name=") {
                            name = n.trim_matches('"').to_owned();
                        }
                    }
                }
            }
            if !name.is_empty() {
                let name_val = vm.alloc_string(name);
                let value_val = vm.alloc_string(value.to_owned());
                let append = vm.alloc_native_fn("FormData.append");
                let _ = vm.invoke_callable(
                    Value::Object(append),
                    fd,
                    &[Value::Object(name_val), Value::Object(value_val)],
                );
            }
        }
    } else if ctype.starts_with("application/x-www-form-urlencoded") {
        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let kd = vm.alloc_string(url_decode(k));
            let vd = vm.alloc_string(url_decode(v));
            let append = vm.alloc_native_fn("FormData.append");
            let _ = vm.invoke_callable(
                Value::Object(append),
                fd,
                &[Value::Object(kd), Value::Object(vd)],
            );
        }
    }
    Ok(fd)
}

/// 轻量百分号解码。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// `Response.formData()` handler 用。
pub(crate) fn response_form_data_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let _ = vm.set_property(receiver, "bodyUsed", Value::Boolean(true));
    let ctype = vm
        .get_property(receiver, "_contentType")
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    parse_form_data_body(vm, &ctype, &text)
}
