//! Headers 全局构造器与实例方法（append / set / get / has / delete / forEach）。

use crate::builtins::{current_receiver, pending_native_name};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

/// Headers 方法名全集。
const HEADERS_METHODS: &[&str] = &[
    "append", "set", "get", "has", "delete", "forEach", "entries", "keys", "values",
];

/// 读取 Headers 有序条目快照 [(name, value)]。
pub(crate) fn hdr_entries(vm: &mut Vm, receiver: Value) -> Vec<(String, String)> {
    let Ok(Value::Object(arr)) = vm.get_property(receiver, "_hdrEntries") else {
        return Vec::new();
    };
    let elements: Vec<Value> = match vm.heap.get(arr.0 as usize) {
        Some(HeapObject::Array { elements, .. }) => elements.clone(),
        _ => Vec::new(),
    };
    elements
        .iter()
        .filter_map(|e| {
            let Value::Object(_) = e else {
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

/// 重建 Headers 条目数组（键统一为小写）。
pub(crate) fn hdr_rewrite(vm: &mut Vm, receiver: Value, entries: &[(String, String)]) {
    let vals: Vec<Value> = entries
        .iter()
        .map(|(name, value)| {
            let entry = vm.alloc_ordinary();
            let n = vm.alloc_string(name.to_ascii_lowercase());
            let _ = vm.set_property(Value::Object(entry), "name", Value::Object(n));
            let v = vm.alloc_string(value.clone());
            let _ = vm.set_property(Value::Object(entry), "value", Value::Object(v));
            Value::Object(entry)
        })
        .collect();
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(receiver, "_hdrEntries", Value::Object(arr));
}

/// 同步小写键属性展示面。
pub(crate) fn hdr_sync_props(vm: &mut Vm, receiver: Value, entries: &[(String, String)]) {
    for (k, v) in entries {
        let s = vm.alloc_string(v.clone());
        let _ = vm.set_property(receiver, &k.to_ascii_lowercase(), Value::Object(s));
    }
}

/// 将任意对象字面量形态规范化为 Headers 实例。
pub(crate) fn build_headers(vm: &mut Vm, val: Value) -> Value {
    if let Some(_) = val.as_object {
        if matches!(vm.get_property(val, "_isHeaders"), Ok(Value::Boolean(true))) {
            return val;
        }
    }
    let h = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(h), "_isHeaders", Value::Boolean(true));
    let ns = vm.alloc_string("Headers".to_owned());
    let _ = vm.set_property(Value::Object(h), "_builtinNs", Value::Object(ns));
    for m in HEADERS_METHODS {
        let f = vm.alloc_native_fn(&format!("Headers.{m}"));
        let _ = vm.set_property(Value::Object(h), m, Value::Object(f));
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(r) = val.as_object {
        for (k, v) in vm.own_entries(r.index()) {
            if !k.starts_with('_') {
                pairs.push((k, vm.format_value(v)));
            }
        }
    }
    hdr_rewrite(vm, Value::Object(h), &pairs);
    hdr_sync_props(vm, Value::Object(h), &pairs);
    Value::Object(h)
}

/// `new Headers([init])`。
pub(crate) fn headers_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let headers = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(headers), "_isHeaders", Value::Boolean(true));
    let ns = vm.alloc_string("Headers".to_owned());
    let _ = vm.set_property(Value::Object(headers), "_builtinNs", Value::Object(ns));
    for method in HEADERS_METHODS {
        let f = vm.alloc_native_fn(&format!("Headers.{method}"));
        let _ = vm.set_property(Value::Object(headers), method, Value::Object(f));
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(r) = args.first().copied().unwrap_or(Value::Undefined).as_object {
        let elements: Vec<Value> = match vm.heap.get(r.index()) {
            Some(HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        };
        if elements.is_empty() {
            for (k, v) in vm.own_entries(r.index()) {
                if k.starts_with('_') {
                    continue;
                }
                entries.push((k, vm.format_value(v)));
            }
        } else {
            for e in elements {
                if let Some(er) = e.as_object {
                    let pair: Vec<Value> = match vm.heap.get(er.index()) {
                        Some(HeapObject::Array { elements, .. }) => elements.clone(),
                        _ => Vec::new(),
                    };
                    if pair.len() >= 2 {
                        entries.push((vm.format_value(pair[0]), vm.format_value(pair[1])));
                    }
                }
            }
        }
    }
    hdr_rewrite(vm, Value::Object(headers), &entries);
    hdr_sync_props(vm, Value::Object(headers), &entries);
    Ok(Value::Object(headers))
}

/// `Headers.get(name)`。
pub(crate) fn headers_get_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(name) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Null);
    };
    let entries = hdr_entries(vm, receiver);
    match entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        .map(|(_, v)| v.clone())
    {
        Some(text) => {
            let s = vm.alloc_string(text);
            Ok(Value::Object(s))
        }
        None => Ok(Value::Null),
    }
}

/// `Headers.has(name)`。
pub(crate) fn headers_has_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(name) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Boolean(false));
    };
    let entries = hdr_entries(vm, receiver);
    Ok(Value::Boolean(
        entries.iter().any(|(k, _)| k.eq_ignore_ascii_case(&name)),
    ))
}

/// Headers 变更类方法统一分派。
pub(crate) fn headers_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = pending_native_name();
    let method = name.rsplit('.').next().unwrap_or("");
    match method {
        "append" => headers_append(vm, args),
        "set" => headers_set(vm, args),
        "delete" => headers_delete(vm, args),
        "forEach" => headers_for_each(vm, args),
        _ => Ok(Value::Undefined),
    }
}

fn headers_append(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(key) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Undefined);
    };
    let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
    let mut entries = hdr_entries(vm, receiver);
    match entries
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case(&key))
    {
        Some(pos) => entries[pos].1 = format!("{}, {}", entries[pos].1, value),
        None => entries.push((key, value)),
    }
    hdr_rewrite(vm, receiver, &entries);
    hdr_sync_props(vm, receiver, &entries);
    Ok(Value::Undefined)
}

fn headers_set(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(key) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Undefined);
    };
    let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
    let mut entries = hdr_entries(vm, receiver);
    match entries
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case(&key))
    {
        Some(pos) => entries[pos].1 = value,
        None => entries.push((key, value)),
    }
    hdr_rewrite(vm, receiver, &entries);
    hdr_sync_props(vm, receiver, &entries);
    Ok(Value::Undefined)
}

fn headers_delete(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(key) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Undefined);
    };
    let entries = hdr_entries(vm, receiver)
        .into_iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(&key))
        .collect::<Vec<_>>();
    hdr_rewrite(vm, receiver, &entries);
    hdr_sync_props(vm, receiver, &entries);
    Ok(Value::Undefined)
}

fn headers_for_each(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(cb) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let entries = hdr_entries(vm, receiver);
    for (name, value) in entries {
        let v = vm.alloc_string(value);
        let k = vm.alloc_string(name);
        let _ = vm.invoke_callable(cb, Value::Undefined, &[Value::Object(v), Value::Object(k)]);
    }
    Ok(Value::Undefined)
}
