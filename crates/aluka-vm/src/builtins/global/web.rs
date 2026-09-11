//! Web 标准全局对象：URLSearchParams / TextEncoder / TextDecoder /
//! ByteLengthQueuingStrategy / CountQueuingStrategy / Blob / Web Streams 全局。

use crate::builtins::{current_receiver, pending_native_name};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

// ===== URLSearchParams =====

/// `new URLSearchParams([init])`。
pub(crate) fn url_search_params_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let usp = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(usp),
        "_isURLSearchParams",
        Value::Boolean(true),
    );
    for method in [
        "append", "get", "getAll", "has", "set", "delete", "toString",
    ] {
        let f = vm.alloc_native_fn(&format!("URLSearchParams.{method}"));
        let _ = vm.set_property(Value::Object(usp), method, Value::Object(f));
    }
    let entries = usp_parse_init(vm, args.first().copied().unwrap_or(Value::Undefined));
    usp_rewrite(vm, Value::Object(usp), &entries);
    Ok(Value::Object(usp))
}

pub(crate) fn usp_parse_init(vm: &mut Vm, init: Value) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let init_text = vm.format_value(init);
    if !init_text.is_empty() && !matches!(init, Value::Undefined | Value::Null | ValueCase::Boolean(_))
    {
        for pair in init_text.split('&').filter(|p| !p.is_empty()) {
            let mut it = pair.splitn(2, '=');
            entries.push((
                it.next().unwrap_or("").to_owned(),
                it.next().unwrap_or("").to_owned(),
            ));
        }
        return entries;
    }
    if let Some(r) = init.as_object() {
        let is_arr = matches!(
            vm.heap.get(r.index()),
            Some(crate::heap::HeapObject::Array { .. })
        );
        if is_arr {
            let elements: Vec<Value> = match vm.heap.get(r.index()) {
                Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
                _ => Vec::new(),
            };
            for e in elements {
                if let Some(er) = e.as_object() {
                    let pair: Vec<Value> = match vm.heap.get(er.index()) {
                        Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
                        _ => Vec::new(),
                    };
                    if pair.len() >= 2 {
                        entries.push((vm.format_value(pair[0]), vm.format_value(pair[1])));
                    }
                }
            }
        } else {
            for (k, v) in vm.own_entries(r.index()) {
                if !k.starts_with('_') {
                    entries.push((k, vm.format_value(v)));
                }
            }
        }
    }
    entries
}

pub(crate) fn usp_entries(vm: &mut Vm, receiver: Value) -> Vec<(String, String)> {
    let Ok(ValueCase::Object(arr)) = vm.get_property(receiver, "_uspEntries") else {
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
            let k = vm
                .get_property(*e, "name")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            let v = vm
                .get_property(*e, "value")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            Some((k, v))
        })
        .collect()
}

pub(crate) fn usp_rewrite(vm: &mut Vm, receiver: Value, entries: &[(String, String)]) {
    let vals: Vec<Value> = entries
        .iter()
        .map(|(n, v)| {
            let o = vm.alloc_ordinary();
            let nref = vm.alloc_string(n.clone());
            let _ = vm.set_property(Value::Object(o), "name", Value::Object(nref));
            let vref = vm.alloc_string(v.clone());
            let _ = vm.set_property(Value::Object(o), "value", Value::Object(vref));
            Value::Object(o)
        })
        .collect();
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(receiver, "_uspEntries", Value::Object(arr));
}

pub(crate) fn url_search_params_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = pending_native_name();
    let method = name.rsplit('.').next().unwrap_or("");
    match method {
        "append" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            let v = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            let mut e = usp_entries(vm, r);
            e.push((k, v));
            usp_rewrite(vm, r, &e);
            Ok(Value::Undefined)
        }
        "get" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            match usp_entries(vm, r)
                .iter()
                .find(|(ek, _)| ek == &k)
                .map(|(_, v)| v.clone())
            {
                Some(s) => Ok(Value::Object(vm.alloc_string(s))),
                None => Ok(Value::Null),
            }
        }
        "getAll" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            let vs: Vec<Value> = usp_entries(vm, r)
                .iter()
                .filter(|(ek, _)| ek == &k)
                .map(|(_, v)| Value::Object(vm.alloc_string(v.clone())))
                .collect();
            Ok(Value::Object(vm.alloc_array(vs)))
        }
        "has" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            Ok(Value::Boolean(
                usp_entries(vm, r).iter().any(|(ek, _)| ek == &k),
            ))
        }
        "set" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            let v = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            let entries = usp_entries(vm, r);
            let pos = entries.iter().position(|(ek, _)| ek == &k);
            let mut e = entries
                .into_iter()
                .filter(|(ek, _)| ek != &k)
                .collect::<Vec<_>>();
            e.insert(pos.unwrap_or(e.len()), (k, v));
            usp_rewrite(vm, r, &e);
            Ok(Value::Undefined)
        }
        "delete" => {
            let r = current_receiver();
            let k = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            let e: Vec<(String, String)> = usp_entries(vm, r)
                .into_iter()
                .filter(|(ek, _)| ek != &k)
                .collect();
            usp_rewrite(vm, r, &e);
            Ok(Value::Undefined)
        }
        "toString" => {
            let r = current_receiver();
            let s: String = usp_entries(vm, r)
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            Ok(Value::Object(vm.alloc_string(s)))
        }
        _ => Ok(Value::Undefined),
    }
}

// ===== TextEncoder =====

/// `new TextEncoder()`。
pub(crate) fn text_encoder_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let te = vm.alloc_ordinary();
    let enc = vm.alloc_string("utf-8".to_owned());
    let _ = vm.set_property(Value::Object(te), "encoding", Value::Object(enc));
    let f = vm.alloc_native_fn("TextEncoder.encode");
    let _ = vm.set_property(Value::Object(te), "encode", Value::Object(f));
    Ok(Value::Object(te))
}

/// `encoder.encode(str)`。
pub(crate) fn text_encoder_encode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let s = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let bytes: Vec<Value> = s
        .as_bytes()
        .iter()
        .map(|b| Value::Number(*b as f64))
        .collect();
    Ok(Value::Object(vm.alloc_array(bytes)))
}

// ===== TextDecoder =====

/// `new TextDecoder([label='utf-8'])`。
pub(crate) fn text_decoder_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let label = args
        .first()
        .map(|v| vm.format_value(*v))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "utf-8".to_owned());
    let td = vm.alloc_ordinary();
    let enc = vm.alloc_string(label);
    let _ = vm.set_property(Value::Object(td), "encoding", Value::Object(enc));
    let f = vm.alloc_native_fn("TextDecoder.decode");
    let _ = vm.set_property(Value::Object(td), "decode", Value::Object(f));
    Ok(Value::Object(td))
}

/// `decoder.decode(uint8)`。
pub(crate) fn text_decoder_decode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut bytes: Vec<u8> = Vec::new();
    if let Some(r) = args.first().and_then(|v| v.as_object()) {
        let elements: Vec<Value> = match vm.heap.get(r.index()) {
            Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        };
        for v in elements {
            if let Some(n) = v.as_number() {
                bytes.push(n as u8);
            }
        }
    }
    let s = String::from_utf8_lossy(&bytes).to_string();
    Ok(Value::Object(vm.alloc_string(s)))
}

// ===== QueuingStrategy =====

/// `new ByteLengthQueuingStrategy(...)` / `new CountQueuingStrategy(...)`。
pub(crate) fn queuing_strategy_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = pending_native_name();
    let strategy = name.rsplit('.').next().unwrap_or("");
    let init = args.first().copied().unwrap_or(Value::Undefined);
    let hwm = match vm.get_property(init, "highWaterMark").map(|v| v.case()).map(ValueCase::from) {
        Ok(ValueCase::Number(n)) => n,
        _ => 1.0,
    };
    let qs = vm.alloc_ordinary();
    let size_fn = vm.alloc_native_fn(&format!("{strategy}.size"));
    let _ = vm.set_property(Value::Object(qs), "size", Value::Object(size_fn));
    let _ = vm.set_property(Value::Object(qs), "highWaterMark", Value::Number(hwm));
    Ok(Value::Object(qs))
}

/// `qs.size(chunk)`。
pub(crate) fn queuing_strategy_size(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = pending_native_name();
    let strategy = name.rsplit('.').next().unwrap_or("");
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    if strategy == "CountQueuingStrategy" {
        return Ok(Value::Number(1.0));
    }
    let len = if let Some(r) = chunk.as_object() {
        match vm.heap.get(r.index()) {
            Some(crate::heap::HeapObject::Array { elements, .. }) => elements.len() as f64,
            _ => 1.0,
        }
    } else {
        1.0
    };
    Ok(Value::Number(len))
}

// ===== Blob =====

/// `new Blob(parts, {type})`。
pub(crate) fn blob_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let parts = args.first().copied().unwrap_or(Value::Undefined);
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);
    let mut text = String::new();
    if let Some(r) = parts.as_object() {
        let elements: Vec<Value> = match vm.heap.get(r.index()) {
            Some(HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        };
        for e in elements {
            let s = vm.format_value(e);
            if let Some(er) = e.as_object() {
                if matches!(vm.heap.get(er.index()), Some(HeapObject::Array { .. })) {
                    let bytes: Vec<Value> = match vm.heap.get(er.index()) {
                        Some(HeapObject::Array { elements, .. }) => elements.clone(),
                        _ => Vec::new(),
                    };
                    let mut sub = String::new();
                    for b in bytes {
                        if let Some(n) = b.as_number() {
                            if let Some(c) = std::char::from_u32(n as u32) {
                                sub.push(c);
                            }
                        }
                    }
                    text.push_str(&sub);
                    continue;
                }
            }
            text.push_str(&s);
        }
    } else if !matches!(parts, Value::Undefined | Value::Null) {
        text.push_str(&vm.format_value(parts));
    }
    let ty = opt_str(vm, opts, "type").unwrap_or_default();
    let blob = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(blob), "_isBlob", Value::Boolean(true));
    let t_ref = vm.alloc_string(ty);
    let _ = vm.set_property(Value::Object(blob), "type", Value::Object(t_ref));
    let s_val = Value::Number(text.len() as f64);
    let _ = vm.set_property(Value::Object(blob), "size", s_val);
    let text_ref = vm.alloc_string(text);
    let _ = vm.set_property(Value::Object(blob), "_blobText", Value::Object(text_ref));
    let f1 = vm.alloc_native_fn("Blob.text");
    let _ = vm.set_property(Value::Object(blob), "text", Value::Object(f1));
    let f2 = vm.alloc_native_fn("Blob.arrayBuffer");
    let _ = vm.set_property(Value::Object(blob), "arrayBuffer", Value::Object(f2));
    Ok(Value::Object(blob))
}

pub(crate) fn blob_text(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let r = current_receiver();
    let _ = vm.set_property(r, "size", Value::Number(0.0));
    let body = vm.get_property(r, "_blobText")?;
    let _ = vm.set_property(r, "size", Value::Number(vm.format_value(body).len() as f64));
    Ok(body)
}

pub(crate) fn blob_array_buffer(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let r = current_receiver();
    let body = vm.get_property(r, "_blobText")?;
    let s = vm.format_value(body);
    let bytes: Vec<Value> = s
        .as_bytes()
        .iter()
        .map(|b| Value::Number(*b as f64))
        .collect();
    Ok(Value::Object(vm.alloc_array(bytes)))
}

// ===== 辅助 =====

/// 读取对象属性并格式化为字符串；`Undefined` / `Null` 视为未提供。
fn opt_str(vm: &mut Vm, obj: Value, key: &str) -> Option<String> {
    let val = vm.get_property(obj, key).ok()?;
    if matches!(val, Value::Undefined | Value::Null) {
        return None;
    }
    let s = vm.format_value(val);
    if s.is_empty() { None } else { Some(s) }
}
