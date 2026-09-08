//! `buffer` 内置模块（Phase 3）：Node Buffer API 基础。
//!
//! 提供 `node:buffer` 模块导出与 Buffer 类：
//! - 模块级导出：`Buffer`、`SlowBuffer`、`kMaxLength`、`constants`、`isUtf8`、`isAscii`；
//! - `Buffer` 静态方法：`from`、`alloc`、`allocUnsafe`、`isBuffer`、`byteLength`、`isEncoding`、`concat`、`compare`；
//! - `Buffer` 实例方法：`toString`、`slice`、`toJSON`、`equals`，以及 `length` 属性和数字索引。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

// Buffer 字节缓存（ObjectRef 索引 -> 字节数组；线程局部：堆句柄仅本线程有效）。
thread_local! {
    static BUFFER_STORE: RefCell<Option<HashMap<u32, Vec<u8>>>> = const { RefCell::new(None) };
}

fn store_buffer(id: u32, data: Vec<u8>) {
    BUFFER_STORE.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, data);
    });
}

fn get_buffer(id: u32) -> Option<Vec<u8>> {
    BUFFER_STORE.with(|g| g.borrow().as_ref()?.get(&id).cloned())
}

/// 提取任意 Value 的底层字节序列（支持 Buffer 实例、字符串、数组等）。
pub fn extract_bytes(vm: &Vm, val: Value) -> Option<Vec<u8>> {
    match val {
        Value::Object(r) => {
            if let Some(bytes) = get_buffer(r.0) {
                return Some(bytes);
            }
            match vm.heap.get(r.0 as usize)? {
                HeapObject::String(s) => Some(s.as_bytes().to_vec()),
                HeapObject::Array { elements, .. } => {
                    let mut bytes = Vec::with_capacity(elements.len());
                    for e in elements {
                        let b = match e {
                            Value::Number(n) => (*n as i64 & 0xFF) as u8,
                            _ => 0,
                        };
                        bytes.push(b);
                    }
                    Some(bytes)
                }
                HeapObject::Ordinary { .. } => {
                    if vm.has_own_slot(r.0 as usize, "_isBuffer") {
                        let len = vm
                            .own_value(r.0 as usize, "length")
                            .and_then(|v| match v {
                                Value::Number(n) => Some(n as usize),
                                _ => None,
                            })
                            .unwrap_or(0);
                        let mut bytes = Vec::with_capacity(len);
                        for i in 0..len {
                            let b = vm
                                .own_value(r.0 as usize, &i.to_string())
                                .and_then(|v| match v {
                                    Value::Number(n) => Some((n as i64 & 0xFF) as u8),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            bytes.push(b);
                        }
                        Some(bytes)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// 在 VM 堆上创建新的 Buffer 实例。
pub fn create_buffer_instance(vm: &mut Vm, data: Vec<u8>) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    store_buffer(obj.0, data.clone());

    let len = data.len();
    let _ = vm.set_property(Value::Object(obj), "length", Value::Number(len as f64));
    let _ = vm.set_property(Value::Object(obj), "_isBuffer", Value::Boolean(true));

    for (i, b) in data.iter().enumerate() {
        let _ = vm.set_property(Value::Object(obj), &i.to_string(), Value::Number(*b as f64));
    }

    let to_string_fn = vm.alloc_native_fn("buffer:instance.toString");
    let slice_fn = vm.alloc_native_fn("buffer:instance.slice");
    let to_json_fn = vm.alloc_native_fn("buffer:instance.toJSON");
    let equals_fn = vm.alloc_native_fn("buffer:instance.equals");

    let _ = vm.set_property(Value::Object(obj), "toString", Value::Object(to_string_fn));
    let _ = vm.set_property(Value::Object(obj), "slice", Value::Object(slice_fn));
    let _ = vm.set_property(Value::Object(obj), "toJSON", Value::Object(to_json_fn));
    let _ = vm.set_property(Value::Object(obj), "equals", Value::Object(equals_fn));

    obj
}

/// 用新字节内容**原地覆盖**既有 Buffer 实例（长度不变；供 `randomFillSync`
/// 等「填充进既有缓冲」的内置库使用，保证 `toString`/索引读到新值）。
pub fn overwrite_buffer_instance(vm: &mut Vm, obj: ObjectRef, data: &[u8]) -> bool {
    // 仅认真实 Buffer 实例
    let known = get_buffer(obj.0).is_some();
    if !known {
        return false;
    }
    store_buffer(obj.0, data.to_vec());
    for (i, b) in data.iter().enumerate() {
        let _ = vm.set_property(Value::Object(obj), &i.to_string(), Value::Number(*b as f64));
    }
    true
}

/// `require("buffer")` / `require("node:buffer")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "buffer",
    build,
};

/// `Buffer` 类对象模块（用于静态方法分派）。
pub const BUFFER_CLASS_MODULE: ModuleDef = ModuleDef {
    name: "Buffer",
    build: build_class,
};

/// `buffer:instance` 实例方法槽位模块。
pub const INSTANCE_MODULE: ModuleDef = ModuleDef {
    name: "buffer:instance",
    build: build_instance,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let buf_class = vm.alloc_native_ctor("Buffer", None);

    // 挂载静态属性与方法
    set_module_prop(vm, obj, "Buffer", Value::Object(buf_class))?;
    set_module_prop(vm, obj, "SlowBuffer", Value::Object(buf_class))?;
    set_module_prop(vm, obj, "kMaxLength", Value::Number((1i64 << 30) as f64))?;

    let constants = vm.alloc_ordinary();
    set_module_prop(vm, obj, "constants", Value::Object(constants))?;

    let is_utf8_fn = vm.alloc_native_fn("buffer.isUtf8");
    let is_ascii_fn = vm.alloc_native_fn("buffer.isAscii");
    set_module_prop(vm, obj, "isUtf8", Value::Object(is_utf8_fn))?;
    set_module_prop(vm, obj, "isAscii", Value::Object(is_ascii_fn))?;

    register_handler(registry, "buffer", "isUtf8", is_utf8);
    register_handler(registry, "buffer", "isAscii", is_ascii);

    // 在 Buffer 类上挂静态函数
    for method in [
        "from",
        "alloc",
        "allocUnsafe",
        "isBuffer",
        "byteLength",
        "isEncoding",
        "concat",
        "compare",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("Buffer.{method}"));
        set_module_prop(vm, buf_class, method, Value::Object(fn_ref))?;
    }

    register_handler(registry, "Buffer", "from", buffer_from);
    register_handler(registry, "Buffer", "alloc", buffer_alloc);
    register_handler(registry, "Buffer", "allocUnsafe", buffer_alloc);
    register_handler(registry, "Buffer", "isBuffer", is_buffer);
    register_handler(registry, "Buffer", "byteLength", byte_length);
    register_handler(registry, "Buffer", "isEncoding", is_encoding);
    register_handler(registry, "Buffer", "concat", concat);
    register_handler(registry, "Buffer", "compare", compare);

    Ok(obj)
}

fn build_class(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let buffer_mod = registry.module("buffer").ok_or_else(|| {
        let msg = vm.alloc_string("buffer 模块尚未初始化".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    let val = vm.get_property(Value::Object(buffer_mod), "Buffer")?;
    match val {
        Value::Object(r) => Ok(r),
        _ => Err(VmError::Thrown(Value::Object(
            vm.alloc_string("Buffer 类缺失".to_owned()),
        ))),
    }
}

fn build_instance(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let slot = vm.alloc_ordinary();
    register_handler(registry, "buffer:instance", "toString", to_string);
    register_handler(registry, "buffer:instance", "slice", slice);
    register_handler(registry, "buffer:instance", "toJSON", to_json);
    register_handler(registry, "buffer:instance", "equals", equals);
    Ok(slot)
}

/// `buffer.isUtf8(buf)`
fn is_utf8(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let Some(bytes) = extract_bytes(vm, *val) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(std::str::from_utf8(&bytes).is_ok()))
}

/// `buffer.isAscii(buf)`
fn is_ascii(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let Some(bytes) = extract_bytes(vm, *val) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(bytes.iter().all(|b| *b < 0x80)))
}

/// `Buffer.isBuffer(val)`
fn is_buffer(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    if let Value::Object(r) = val {
        if get_buffer(r.0).is_some() {
            return Ok(Value::Boolean(true));
        }
        if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Ordinary { .. })) {
            return Ok(Value::Boolean(vm.has_own_slot(r.0 as usize, "_isBuffer")));
        }
    }
    Ok(Value::Boolean(false))
}

/// `Buffer.isEncoding(encoding)`
fn is_encoding(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let enc = args
        .first()
        .map(|v| vm.format_value(*v).to_lowercase())
        .unwrap_or_default();
    let supported = matches!(
        enc.as_str(),
        "utf8" | "utf-8" | "hex" | "base64" | "latin1" | "binary" | "ascii"
    );
    Ok(Value::Boolean(supported))
}

/// `Buffer.byteLength(str, [encoding])`
fn byte_length(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(val) = args.first() else {
        return Ok(Value::Number(0.0));
    };
    if let Some(bytes) = extract_bytes(vm, *val) {
        return Ok(Value::Number(bytes.len() as f64));
    }
    let s = vm.format_value(*val);
    let enc = args
        .get(1)
        .map(|v| vm.format_value(*v).to_lowercase())
        .unwrap_or_else(|| "utf8".to_owned());
    let bytes = decode_string_to_bytes(&s, &enc);
    Ok(Value::Number(bytes.len() as f64))
}

/// `Buffer.from(input, [encoding])`
fn buffer_from(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        let inst = create_buffer_instance(vm, Vec::new());
        return Ok(Value::Object(inst));
    }
    let input = args[0];
    let enc = args
        .get(1)
        .map(|v| vm.format_value(*v).to_lowercase())
        .unwrap_or_else(|| "utf8".to_owned());

    match input {
        Value::Object(r) => {
            // 字符串实参：按 encoding 解码（extract_bytes 对字符串会退回
            // 原始 UTF-8 字节，绕过 hex/base64 转换，必须先行分流）
            if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::String(_))) {
                let s = vm.format_value(input);
                let bytes = decode_string_to_bytes(&s, &enc);
                let inst = create_buffer_instance(vm, bytes);
                return Ok(Value::Object(inst));
            }
            if let Some(bytes) = extract_bytes(vm, Value::Object(r)) {
                let inst = create_buffer_instance(vm, bytes);
                return Ok(Value::Object(inst));
            }
            let s = vm.format_value(input);
            let bytes = decode_string_to_bytes(&s, &enc);
            let inst = create_buffer_instance(vm, bytes);
            Ok(Value::Object(inst))
        }
        _ => {
            let s = vm.format_value(input);
            let bytes = decode_string_to_bytes(&s, &enc);
            let inst = create_buffer_instance(vm, bytes);
            Ok(Value::Object(inst))
        }
    }
}

/// `Buffer.alloc(size, [fill])`
fn buffer_alloc(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let size = args
        .first()
        .and_then(|v| match v {
            Value::Number(n) => Some((*n as i64).max(0) as usize),
            _ => None,
        })
        .unwrap_or(0);

    let fill_byte = if let Some(fill) = args.get(1) {
        match fill {
            Value::Number(n) => (*n as i64 & 0xFF) as u8,
            _ => {
                let s = vm.format_value(*fill);
                s.as_bytes().first().copied().unwrap_or(0)
            }
        }
    } else {
        0
    };

    let data = vec![fill_byte; size];
    let inst = create_buffer_instance(vm, data);
    Ok(Value::Object(inst))
}

/// `Buffer.concat(list, [totalLength])`
fn concat(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(Value::Object(arr_ref)) = args.first() else {
        let inst = create_buffer_instance(vm, Vec::new());
        return Ok(Value::Object(inst));
    };

    let elems = match vm.heap.get(arr_ref.0 as usize) {
        Some(HeapObject::Array { elements, .. }) => elements.clone(),
        _ => Vec::new(),
    };

    let mut all_bytes = Vec::new();
    for e in elems {
        if let Some(bytes) = extract_bytes(vm, e) {
            all_bytes.extend(bytes);
        }
    }

    if let Some(Value::Number(n)) = args.get(1) {
        let total = (*n as i64).max(0) as usize;
        all_bytes.truncate(total);
    }

    let inst = create_buffer_instance(vm, all_bytes);
    Ok(Value::Object(inst))
}

/// `Buffer.compare(a, b)`
fn compare(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let a_bytes = args
        .first()
        .and_then(|v| extract_bytes(vm, *v))
        .unwrap_or_default();
    let b_bytes = args
        .get(1)
        .and_then(|v| extract_bytes(vm, *v))
        .unwrap_or_default();
    let res = a_bytes.cmp(&b_bytes);
    let n = match res {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    };
    Ok(Value::Number(n))
}

/// 实例方法：`buf.toString([encoding, start, end])`
fn to_string(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this_val = current_receiver();
    let bytes = extract_bytes(vm, this_val).unwrap_or_default();
    let enc = args
        .first()
        .map(|v| vm.format_value(*v).to_lowercase())
        .unwrap_or_else(|| "utf8".to_owned());

    let start = args
        .get(1)
        .and_then(|v| match v {
            Value::Number(n) => Some((*n as i64).max(0) as usize),
            _ => None,
        })
        .unwrap_or(0)
        .min(bytes.len());

    let end = args
        .get(2)
        .and_then(|v| match v {
            Value::Number(n) => Some((*n as i64).max(0) as usize),
            _ => None,
        })
        .unwrap_or(bytes.len())
        .min(bytes.len());

    let slice = if start < end { &bytes[start..end] } else { &[] };
    let out = encode_bytes_to_string(slice, &enc);
    let s_ref = vm.alloc_string(out);
    Ok(Value::Object(s_ref))
}

/// 实例方法：`buf.slice([start, end])`
fn slice(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this_val = current_receiver();
    let bytes = extract_bytes(vm, this_val).unwrap_or_default();
    let len = bytes.len() as i64;

    let start = args
        .first()
        .and_then(|v| match v {
            Value::Number(n) => {
                let n = *n as i64;
                if n < 0 {
                    Some((len + n).max(0) as usize)
                } else {
                    Some(n.min(len) as usize)
                }
            }
            _ => None,
        })
        .unwrap_or(0);

    let end = args
        .get(1)
        .and_then(|v| match v {
            Value::Number(n) => {
                let n = *n as i64;
                if n < 0 {
                    Some((len + n).max(0) as usize)
                } else {
                    Some(n.min(len) as usize)
                }
            }
            _ => None,
        })
        .unwrap_or(bytes.len());

    let sliced = if start < end && start < bytes.len() {
        bytes[start..end.min(bytes.len())].to_vec()
    } else {
        Vec::new()
    };

    let inst = create_buffer_instance(vm, sliced);
    Ok(Value::Object(inst))
}

/// 实例方法：`buf.toJSON()`
fn to_json(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this_val = current_receiver();
    let bytes = extract_bytes(vm, this_val).unwrap_or_default();

    let obj = vm.alloc_ordinary();
    let type_str = vm.alloc_string("Buffer".to_owned());
    let _ = vm.set_property(Value::Object(obj), "type", Value::Object(type_str));

    let elems: Vec<Value> = bytes.into_iter().map(|b| Value::Number(b as f64)).collect();
    let arr = vm.alloc_array(elems);
    let _ = vm.set_property(Value::Object(obj), "data", Value::Object(arr));

    Ok(Value::Object(obj))
}

/// 实例方法：`buf.equals(other)`
fn equals(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this_val = current_receiver();
    let a_bytes = extract_bytes(vm, this_val).unwrap_or_default();
    let b_bytes = args
        .first()
        .and_then(|v| extract_bytes(vm, *v))
        .unwrap_or_default();
    Ok(Value::Boolean(a_bytes == b_bytes))
}

// 内部编解码辅助工具
fn decode_string_to_bytes(s: &str, enc: &str) -> Vec<u8> {
    match enc {
        "hex" => {
            let mut bytes = Vec::new();
            let hex_chars: Vec<char> = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            for chunk in hex_chars.chunks(2) {
                if chunk.len() == 2 {
                    let hex_str: String = chunk.iter().collect();
                    if let Ok(b) = u8::from_str_radix(&hex_str, 16) {
                        bytes.push(b);
                    }
                }
            }
            bytes
        }
        "base64" => {
            // 基础 base64 解码，容错处理
            aluka_base64_decode(s).unwrap_or_else(|| s.as_bytes().to_vec())
        }
        "latin1" | "binary" | "ascii" => s.chars().map(|c| (c as u32 & 0xFF) as u8).collect(),
        _ => s.as_bytes().to_vec(),
    }
}

fn encode_bytes_to_string(bytes: &[u8], enc: &str) -> String {
    match enc {
        "hex" => {
            let mut s = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                s.push_str(&format!("{b:02x}"));
            }
            s
        }
        "base64" => aluka_base64_encode(bytes),
        "latin1" | "binary" | "ascii" => bytes.iter().map(|b| *b as char).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn aluka_base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = if i + 1 < data.len() { data[i + 1] } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] } else { 0 };

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        if i + 1 < data.len() {
            out.push(TABLE[(((b1 & 0xF) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(TABLE[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

fn aluka_base64_decode(s: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let val_of = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < clean.len() {
        let c0 = val_of(clean[i])?;
        let c1 = if i + 1 < clean.len() {
            val_of(clean[i + 1])?
        } else {
            0
        };
        out.push((c0 << 2) | (c1 >> 4));
        if i + 2 < clean.len() {
            let c2 = val_of(clean[i + 2])?;
            out.push(((c1 & 0xF) << 4) | (c2 >> 2));
            if i + 3 < clean.len() {
                let c3 = val_of(clean[i + 3])?;
                out.push(((c2 & 3) << 6) | c3);
            }
        }
        i += 4;
    }
    Some(out)
}
