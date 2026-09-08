//! 对象属性读写、访问器触发、原型链遍历与 Instanceof 语义。

use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::{Vm, VmError};
use crate::jit_helpers::{from_vm_value, to_vm_value};
use crate::ops::to_number;
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;

/// flags 规范序（JS canonical order 子集）。
fn canonical_regexp_flags(flags: &str) -> String {
    const ORDER: [char; 7] = ['d', 'g', 'i', 'm', 's', 'u', 'y'];
    ORDER.iter().filter(|c| flags.contains(**c)).collect()
}

impl Vm {
    /// 读取 Ordinary 对象自有属性值（未删除；快速/字典两模式统一入口）。
    ///
    /// 快速模式：隐藏类命中 → O(1) 槽位直读；字典模式：HashMap 查值。
    /// 删除集先于存储判定（删除不改 shape）。
    pub(crate) fn own_value(&self, idx: usize, key: &str) -> Option<Value> {
        let HeapObject::Ordinary { props, deleted, .. } = self.heap.get(idx)? else {
            return None;
        };
        if deleted.contains(key) {
            return None;
        }
        match props {
            OrdinaryProps::Shape { shape, slots } => {
                let slot = self.shape_table.shape(*shape)?.lookup(key)?;
                slots.get(slot).map(|&b| to_vm_value(b))
            }
            OrdinaryProps::Dict { properties } => properties.get(key).copied(),
        }
    }

    /// Ordinary 对象是否含自有属性（未删除）。
    pub(crate) fn has_own_slot(&self, idx: usize, key: &str) -> bool {
        self.own_value(idx, key).is_some()
    }

    /// 枚举 Ordinary 对象自有属性（键 + 值，快速模式为槽位序 = 插入序；
    /// 字典模式为哈希序；均跳过删除项）。
    pub(crate) fn own_entries(&self, idx: usize) -> Vec<(String, Value)> {
        let Some(HeapObject::Ordinary { props, deleted, .. }) = self.heap.get(idx) else {
            return Vec::new();
        };
        match props {
            OrdinaryProps::Shape { shape, slots } => {
                let Some(s) = self.shape_table.shape(*shape) else {
                    return Vec::new();
                };
                let mut out = Vec::with_capacity(s.len());
                for (i, name) in s.names().enumerate() {
                    if deleted.contains(name) {
                        continue;
                    }
                    out.push((
                        name.to_owned(),
                        slots
                            .get(i)
                            .map(|&b| to_vm_value(b))
                            .unwrap_or(Value::Undefined),
                    ));
                }
                out
            }
            OrdinaryProps::Dict { properties } => properties
                .iter()
                .filter(|(k, _)| !deleted.contains(*k))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    /// 删除 Ordinary 对象的自有属性（快速模式清槽 + 记入删除集 + 代数递增，
    /// 字典模式直接移除；shape 语义与哈希语义统一为「删除后不可见」）。
    /// 非 Ordinary 或无该属性时为无操作。Proxy 对象经 deleteProperty trap 派发。
    pub(crate) fn delete_property(&mut self, obj: Value, key: &str) {
        // Proxy 对象：经 deleteProperty trap 派发（假值抛 TypeError 由 trap 层处理）
        if let Value::Object(r) = obj {
            if self.proxy_parts(r).is_some() {
                let _ = self.proxy_delete(r, key);
                return;
            }
        }
        if let Value::Object(r) = obj {
            if let Some(HeapObject::Ordinary {
                props,
                deleted,
                deleted_gen,
                ..
            }) = self.heap.get_mut(r.0 as usize)
            {
                match props {
                    OrdinaryProps::Shape { shape, slots } => {
                        if let Some(slot) =
                            self.shape_table.shape(*shape).and_then(|s| s.lookup(key))
                        {
                            if slot < slots.len() {
                                slots[slot] = aluka_jit::valbox::UNDEFINED;
                            }
                        }
                    }
                    OrdinaryProps::Dict { properties } => {
                        properties.remove(key);
                    }
                }
                deleted.insert(key.to_owned());
                *deleted_gen += 1;
            }
        }
    }

    /// 获取常量池字符串。
    pub fn get_const_string(&self, idx: usize) -> Result<String, VmError> {
        Ok(self
            .const_string(idx)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{idx}")))
    }

    /// 借用当前常量池中的字符串，供解释循环热路径使用。
    ///
    /// 与公共兼容 API `get_const_string` 不同，这个方法不复制字符串；返回的
    /// 借用只应在下一次可变 VM 操作前使用。非法/非字符串常量返回 `None`，
    /// 由调用点按原有格式化回退处理。
    pub(crate) fn const_string(&self, idx: usize) -> Option<&str> {
        match self.current_constants.get(idx) {
            Some(aluka_bytecode::Constant::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// 将任意值转换为属性键。
    pub fn to_property_key(&self, val: Value) -> String {
        match val {
            Value::Number(n) => {
                if n.fract() == 0.0 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            }
            Value::Boolean(b) => format!("{b}"),
            Value::Null => "null".to_owned(),
            Value::Undefined => "undefined".to_owned(),
            Value::Object(r) => {
                let idx = r.0 as usize;
                if idx < self.heap.len() {
                    match &self.heap[idx] {
                        HeapObject::String(s) => return s.clone(),
                        HeapObject::BigInt(s) => return s.clone(),
                        HeapObject::Symbol { .. } => return crate::symbol::mangled_key(r),
                        _ => {}
                    }
                }
                if let Some(aluka_bytecode::Constant::String(s)) = self.current_constants.get(idx) {
                    s.clone()
                } else {
                    format!("[Object {:?}]", r)
                }
            }
        }
    }

    /// 读取属性（含原型链查找、getter 触发与数组元素读取）。
    pub fn get_property(&mut self, obj: Value, key: &str) -> Result<Value, VmError> {
        // Proxy 对象：经 get trap 派发（含 revoked 校验与 target 回退）
        if let Value::Object(r) = obj {
            if self.proxy_parts(r).is_some() {
                return self.proxy_get(r, key, obj);
            }
        }
        // globalThis：属性读取直通全局变量表与内建全局
        if let Value::Object(r) = obj {
            if self.has_own_slot(r.0 as usize, "_isGlobalThis") {
                return Ok(self.resolve_global(key));
            }
        }
        // 内置对象的方法按需物化（process.nextTick 等属性访问先于调用）
        if key == "env" && self.process_object.is_some_and(|p| obj == Value::Object(p)) {
            // process.env：物化为环境变量对象
            let env_obj = self.alloc_ordinary();
            for (k, v) in std::env::vars() {
                let s_ref = self.alloc_string(v);
                let _ = self.set_property(Value::Object(env_obj), &k, Value::Object(s_ref));
            }
            return Ok(Value::Object(env_obj));
        }
        if key == "nextTick" && self.process_object.is_some_and(|p| obj == Value::Object(p)) {
            return Ok(Value::Object(self.alloc_native_fn("nextTick")));
        }
        // Symbol 构造器的知名符号物化（Symbol.iterator 等属性读取）
        if self.is_native_fn(obj, "Symbol") && crate::symbol::WELL_KNOWN_NAMES.contains(&key) {
            return Ok(self.well_known_symbol(key));
        }
        // 闭包函数：`name` / `length` 读模板元数据（Go 前端编译产物携带函数名）。
        // 先判断键再取模板：普通属性（尤其热路径中的 `prototype`/自定义键）
        // 不需要复制函数名 String。
        if let Value::Object(r) = obj {
            let func_idx = match self.heap.get(r.0 as usize) {
                Some(HeapObject::Closure { func_idx, .. }) => Some(*func_idx),
                _ => None,
            };
            if key == "length" {
                if let Some(num_params) = func_idx
                    .and_then(|idx| self.module_functions.get(idx))
                    .map(|t| t.num_params)
                {
                    return Ok(Value::Number(num_params as f64));
                }
            } else if key == "name" {
                if let Some(name) = func_idx
                    .and_then(|idx| self.module_functions.get(idx))
                    .map(|t| t.name.clone())
                {
                    return Ok(Value::Object(self.alloc_string(name)));
                }
            }
        }
        // 字符串接收者：`length` 与数字下标访问（原型方法由 CALL_METHOD 链求值）
        if let Value::Object(r) = obj {
            let str_len = match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(text)) => Some(text.chars().count()),
                _ => None,
            };
            if let Some(len) = str_len {
                if key == "length" {
                    return Ok(Value::Number(len as f64));
                }
                if let Ok(i) = key.parse::<usize>() {
                    if i < len {
                        if let Some(HeapObject::String(text)) = self.heap.get(r.0 as usize) {
                            let ch = text
                                .chars()
                                .nth(i)
                                .map(|c| c.to_string())
                                .unwrap_or_default();
                            return Ok(Value::Object(self.alloc_string(ch)));
                        }
                    }
                    return Ok(Value::Undefined);
                }
            }
        }
        let mut cur = obj;
        let mut depth = 0;
        while let Value::Object(r) = cur {
            if depth > 100 {
                break;
            }
            depth += 1;
            let idx = r.0 as usize;
            if idx >= self.heap.len() {
                break;
            }
            match &self.heap[idx] {
                HeapObject::Ordinary { getters, proto, .. } => {
                    if let Some(&g_idx) = getters.get(key) {
                        return self.invoke_function(g_idx, obj, &[], Vec::new());
                    }
                    if let Some(v) = self.own_value(idx, key) {
                        return Ok(v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else {
                        break;
                    }
                }
                HeapObject::Closure {
                    properties, proto, ..
                } => {
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else {
                        break;
                    }
                }
                HeapObject::NativeCtor { properties, .. } => {
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    break;
                }
                HeapObject::NativeFn { properties, .. } => {
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    break;
                }
                HeapObject::Array {
                    elements,
                    properties,
                    proto,
                } => {
                    if key == "length" {
                        return Ok(Value::Number(elements.len() as f64));
                    }
                    if let Ok(i) = key.parse::<usize>() {
                        return Ok(elements.get(i).copied().unwrap_or(Value::Undefined));
                    }
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        // RegExp 实例表面（source/flags 系/lastIndex/constructor）：堆对象
        // 不携带属性表，按需合成（`lastIndex` 存线程局部状态表）
        if let Value::Object(r) = obj {
            if let Some(HeapObject::RegExp { pattern, flags }) = self.heap.get(r.0 as usize) {
                let (pattern, flags) = (pattern.clone(), flags.clone());
                let ctor = self.regexp_ctor;
                let synthesized = match key {
                    "source" => Some(Value::Object(self.alloc_string(if pattern.is_empty() {
                        "(?:)".to_owned()
                    } else {
                        pattern
                    }))),
                    "flags" => Some(Value::Object(
                        self.alloc_string(canonical_regexp_flags(&flags)),
                    )),
                    "global" => Some(Value::Boolean(flags.contains('g'))),
                    "ignoreCase" => Some(Value::Boolean(flags.contains('i'))),
                    "multiline" => Some(Value::Boolean(flags.contains('m'))),
                    "dotAll" => Some(Value::Boolean(flags.contains('s'))),
                    "sticky" => Some(Value::Boolean(flags.contains('y'))),
                    "lastIndex" => Some(Value::Number(
                        crate::interpreter::regex_last_index(r.0) as f64
                    )),
                    "constructor" => ctor.map(Value::Object),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
        }
        // Map/Set 实例的 size 属性（entries 数）
        if let Value::Object(r) = obj {
            if let Some(HeapObject::Map { entries }) = self.heap.get(r.0 as usize) {
                if key == "size" {
                    return Ok(Value::Number(entries.len() as f64));
                }
            }
        }
        // TypedArray / DataView / ArrayBuffer 实例表面（length/buffer 等按需合成）
        if let Value::Object(r) = obj {
            // 先快照堆字段（避免可变借用与堆读取冲突）
            let ta_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::TypedArray {
                    kind,
                    buffer,
                    byte_offset,
                    length,
                }) => Some((*kind, *buffer, *byte_offset, *length)),
                _ => None,
            };
            if let Some((kind, buffer, byte_offset, length)) = ta_info {
                // 数值下标 → 元素读取（越界 undefined）
                if let Ok(i) = key.parse::<usize>() {
                    if i < length {
                        let off = byte_offset + i * kind.elem_size();
                        self.check_detached(buffer)?;
                        let Some(HeapObject::ArrayBuffer { data, .. }) =
                            self.heap.get(buffer.0 as usize)
                        else {
                            return Ok(Value::Undefined);
                        };
                        let elem = kind.read_le(data, off);
                        return Ok(self.decode_element(kind, elem));
                    }
                    return Ok(Value::Undefined);
                }
                let synthesized = match key {
                    "length" => Some(Value::Number(length as f64)),
                    "byteLength" => Some(Value::Number((length * kind.elem_size()) as f64)),
                    "byteOffset" => Some(Value::Number(byte_offset as f64)),
                    "buffer" => Some(Value::Object(buffer)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
            let dv_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::DataView {
                    buffer,
                    byte_offset,
                    byte_length,
                }) => Some((*buffer, *byte_offset, *byte_length)),
                _ => None,
            };
            if let Some((buffer, byte_offset, byte_length)) = dv_info {
                let synthesized = match key {
                    "byteLength" => Some(Value::Number(byte_length as f64)),
                    "byteOffset" => Some(Value::Number(byte_offset as f64)),
                    "buffer" => Some(Value::Object(buffer)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
            let ab_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::ArrayBuffer {
                    data,
                    resizable,
                    max_byte_length,
                    detached,
                    ..
                }) => Some((data.len(), *resizable, *max_byte_length, *detached)),
                _ => None,
            };
            if let Some((len, resizable, max_len, detached)) = ab_info {
                let synthesized = match key {
                    "byteLength" => Some(Value::Number(len as f64)),
                    "detached" => Some(Value::Boolean(detached)),
                    "resizable" => Some(Value::Boolean(resizable)),
                    "maxByteLength" => Some(Value::Number(max_len.max(len) as f64)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
        }
        // Object.prototype.hasOwnProperty：不落于原型对象（保持零自有
        // 属性，for-in 口径对齐 Node.js 22 LTS 标准），属性链查不到时在此合成
        if key == "hasOwnProperty" {
            if let Some(h) = self.objproto_has_own {
                return Ok(Value::Object(h));
            }
        }
        // null/undefined 上读属性：JS 语义抛 TypeError（Node 22 消息形态）
        if matches!(obj, Value::Undefined | Value::Null) {
            let kind = if obj == Value::Null {
                "null"
            } else {
                "undefined"
            };
            let err = self.alloc_error_instance(&format!(
                "Cannot read properties of {kind} (reading '{key}')"
            ));
            let name = self.alloc_string("TypeError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            return Err(VmError::Thrown(Value::Object(err)));
        }
        Ok(Value::Undefined)
    }

    /// 设置属性（含数组下标写入、闭包对象属性写入与 Setter 访问器触发）。
    pub fn set_property(&mut self, obj: Value, key: &str, val: Value) -> Result<(), VmError> {
        // Proxy 对象：经 set trap 派发（假值返回抛 TypeError）
        if let Value::Object(r) = obj {
            if self.proxy_parts(r).is_some() {
                return self.proxy_set(r, key, val, obj);
            }
        }
        // globalThis：属性写入直通全局变量表
        if let Value::Object(r) = obj {
            if self.has_own_slot(r.0 as usize, "_isGlobalThis") {
                self.globals.insert(key.to_owned(), val);
                return Ok(());
            }
        }
        // RegExp 实例的 lastIndex：写线程局部状态表（堆对象无可变属性）
        if key == "lastIndex" {
            if let Value::Object(r) = obj {
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::RegExp { .. })) {
                    crate::interpreter::set_regex_last_index(r.0, to_number(val).max(0.0) as usize);
                    return Ok(());
                }
            }
        }
        // TypedArray 数值下标写入：按元素类型收窄/钳制后落盘（越界忽略）
        if let Value::Object(r) = obj {
            if let Ok(i) = key.parse::<usize>() {
                if matches!(
                    self.heap.get(r.0 as usize),
                    Some(HeapObject::TypedArray { .. })
                ) {
                    return self.ta_set(r, i, val);
                }
            }
        }
        if let Value::Object(r) = obj {
            let idx = r.0 as usize;
            if idx < self.heap.len() {
                // Setter 访问器优先：命中则调用（不写数据属性，对齐 JS [[Set]] 语义）
                let setter = match &self.heap[idx] {
                    HeapObject::Ordinary { setters, .. } => setters.get(key).copied(),
                    _ => None,
                };
                if let Some(s_idx) = setter {
                    self.invoke_function(s_idx, obj, &[val], Vec::new())?;
                    return Ok(());
                }
                match &mut self.heap[idx] {
                    HeapObject::Ordinary { props, deleted, .. } => {
                        // 曾删除的 key：清除删除标记后直写（快速模式 shape 仍保留槽位）
                        if !deleted.is_empty() {
                            deleted.remove(key);
                        }
                        let next: Option<OrdinaryProps> = match props {
                            OrdinaryProps::Shape { shape, slots } => {
                                match self.shape_table.shape(*shape).and_then(|s| s.lookup(key)) {
                                    Some(slot) if slot < slots.len() => {
                                        slots[slot] = from_vm_value(val);
                                        None
                                    }
                                    // 新属性且已达字典阈值：整体转字典模式，
                                    // 避免 shape transition 树对海量键 O(n²) 克隆
                                    _ if slots.len() >= crate::heap::DICT_THRESHOLD => {
                                        let mut properties =
                                            HashMap::with_capacity(slots.len() + 1);
                                        if let Some(s) = self.shape_table.shape(*shape) {
                                            for (i, name) in s.names().enumerate() {
                                                // 盒 → VM Value 物化进字典
                                                properties.insert(
                                                    name.to_owned(),
                                                    slots
                                                        .get(i)
                                                        .map(|&b| to_vm_value(b))
                                                        .unwrap_or(Value::Undefined),
                                                );
                                            }
                                        }
                                        properties.insert(key.to_owned(), val);
                                        Some(OrdinaryProps::Dict { properties })
                                    }
                                    _ => {
                                        // 新属性：沿 shape transition 派生子隐藏类，追加槽位
                                        let new_shape = self.shape_table.transition(*shape, key);
                                        slots.push(from_vm_value(val));
                                        *shape = new_shape;
                                        None
                                    }
                                }
                            }
                            OrdinaryProps::Dict { properties } => {
                                properties.insert(key.to_owned(), val);
                                None
                            }
                        };
                        if let Some(next) = next {
                            *props = next;
                        }
                    }
                    HeapObject::Closure { properties, .. } => {
                        properties.insert(key.to_owned(), val);
                    }
                    HeapObject::NativeCtor { properties, .. } => {
                        properties.insert(key.to_owned(), val);
                    }
                    HeapObject::Array {
                        elements,
                        properties,
                        ..
                    } => {
                        if let Ok(i) = key.parse::<usize>() {
                            if i >= elements.len() {
                                elements.resize(i + 1, Value::Undefined);
                            }
                            elements[i] = val;
                        } else if key != "length" {
                            properties.insert(key.to_owned(), val);
                        }
                    }
                    _ => {}
                }
                // 写屏障：老容器写入年轻引用 → 记忆集（minor GC 次级根）
                self.gc_write_barrier(r, val);
            }
        }
        Ok(())
    }

    /// 判断属性（自有或沿原型链）是否存在于对象上（`in` 运算符语义）。
    pub fn has_property(&mut self, obj: Value, key: &str) -> bool {
        // Proxy 对象：经 has trap 派发
        if let Value::Object(r) = obj {
            if self.proxy_parts(r).is_some() {
                return self.proxy_has(r, key).unwrap_or(false);
            }
        }
        let mut cur = obj;
        let mut depth = 0;
        while let Value::Object(r) = cur {
            if depth > 100 {
                break;
            }
            depth += 1;
            let idx = r.0 as usize;
            if idx >= self.heap.len() {
                break;
            }
            match &self.heap[idx] {
                HeapObject::Ordinary {
                    getters,
                    setters,
                    proto,
                    ..
                } => {
                    if getters.contains_key(key) || setters.contains_key(key) {
                        return true;
                    }
                    if self.own_value(idx, key).is_some() {
                        return true;
                    }
                    cur = match proto {
                        Some(p) => Value::Object(*p),
                        None => break,
                    };
                }
                HeapObject::Closure { properties, .. }
                | HeapObject::NativeCtor { properties, .. } => {
                    if properties.contains_key(key) {
                        return true;
                    }
                    break;
                }
                HeapObject::Array { elements, .. } => {
                    if key == "length" {
                        return true;
                    }
                    if let Ok(i) = key.parse::<usize>() {
                        return i < elements.len();
                    }
                    break;
                }
                _ => break,
            }
        }
        false
    }

    /// 枚举对象自有属性（键 + 值），供 `{ ...src }` 展开使用。
    ///
    /// 普通对象取属性字典；数组产出索引键与 `length`；Proxy 经 ownKeys +
    /// get trap 派发。其余类型为空集。
    pub(crate) fn own_properties(&mut self, obj: Value) -> Vec<(String, Value)> {
        // Proxy 对象：ownKeys trap 列键、get trap 取值（规范 [[OwnPropertyKeys]]）
        if let Value::Object(r) = obj {
            if self.proxy_parts(r).is_some() {
                let keys = self.proxy_own_keys(r).unwrap_or_default();
                return keys
                    .into_iter()
                    .map(|k| {
                        let v = self.get_property(obj, &k).unwrap_or(Value::Undefined);
                        (k, v)
                    })
                    .collect();
            }
        }
        if let Value::Object(r) = obj {
            let idx = r.0 as usize;
            if idx < self.heap.len() {
                match &self.heap[idx] {
                    HeapObject::Ordinary { .. } => {
                        return self.own_entries(idx);
                    }
                    HeapObject::Array { elements, .. } => {
                        let mut out = Vec::with_capacity(elements.len() + 1);
                        for (i, v) in elements.iter().enumerate() {
                            out.push((i.to_string(), *v));
                        }
                        out.push(("length".to_owned(), Value::Number(elements.len() as f64)));
                        return out;
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    /// `for-in` 键枚举（对齐 Go 版 `EnumerateForInKeys`）。
    ///
    /// 沿原型链（≤128 层）收集自有键并去重（先到先得，自有键优先）；
    /// 字符串产出索引键（按 UTF-16 code unit 计）；原始值为空集。
    pub(crate) fn enumerate_for_in_keys(&self, val: Value) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = Some(val);
        for _ in 0..128 {
            let Some(v) = cur.take() else { break };
            let Value::Object(r) = v else { break };
            let idx = r.0 as usize;
            let Some(h) = self.heap.get(idx) else { break };
            let (keys, proto) = match h {
                HeapObject::Ordinary {
                    props,
                    deleted,
                    proto,
                    ..
                } => {
                    let ks = match props {
                        OrdinaryProps::Shape { shape, .. } => self
                            .shape_table
                            .shape(*shape)
                            .map(|s| {
                                s.names()
                                    .filter(|n| !deleted.contains(*n))
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                        OrdinaryProps::Dict { properties } => properties
                            .keys()
                            .filter(|k| !deleted.contains(*k))
                            .cloned()
                            .collect(),
                    };
                    (ks, *proto)
                }
                HeapObject::Array {
                    elements,
                    properties,
                    proto,
                } => {
                    let mut ks = (0..elements.len())
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>();
                    ks.extend(properties.keys().cloned());
                    (ks, *proto)
                }
                HeapObject::Closure { properties, .. } => {
                    (properties.keys().cloned().collect::<Vec<_>>(), None)
                }
                HeapObject::String(s) => {
                    // 索引键按 UTF-16 code unit 计（星面字符占 2 个）
                    let units: usize = s.chars().map(|c| if c > '\u{FFFF}' { 2 } else { 1 }).sum();
                    let ks = (0..units).map(|i| i.to_string()).collect::<Vec<_>>();
                    (ks, None)
                }
                _ => (Vec::new(), None),
            };
            for k in keys {
                if seen.insert(k.clone()) {
                    out.push(k);
                }
            }
            cur = proto.map(Value::Object);
        }
        out
    }

    /// 读取对象的内部原型 [[Prototype]]。
    pub fn get_prototype(&self, val: Value) -> Option<ObjectRef> {
        if let Value::Object(r) = val {
            let idx = r.0 as usize;
            if let Some(obj) = self.heap.get(idx) {
                match obj {
                    HeapObject::Ordinary { proto, .. } => *proto,
                    HeapObject::Closure { proto, .. } => *proto,
                    HeapObject::Array { proto, .. } => *proto,
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// 写入对象的内部原型 [[Prototype]]（`Object.setPrototypeOf` 底层语义；
    /// Proxy 由 [`crate::proxy`] 的 trap 路径先行拦截，此处仅处理普通容器）。
    /// 非 Ordinary/Array/Closure 对象为无操作。
    pub(crate) fn set_prototype_of(&mut self, obj: Value, proto: Option<ObjectRef>) {
        if let Value::Object(r) = obj {
            if let Some(
                HeapObject::Ordinary { proto: p, .. }
                | HeapObject::Closure { proto: p, .. }
                | HeapObject::Array { proto: p, .. },
            ) = self.heap.get_mut(r.0 as usize)
            {
                *p = proto;
            }
        }
    }

    /// 合成对象属性的特性描述对象（`Object.getOwnPropertyDescriptor` 底层）。
    ///
    /// 本运行时数据属性恒为可写/可枚举/可配置（无属性位存储）；访问器经
    /// Ordinary 的 getter/setter 表判定。属性不存在时返回 undefined。
    pub(crate) fn ordinary_property_descriptor(
        &mut self,
        obj: Value,
        key: &str,
    ) -> Result<Value, VmError> {
        if !self.has_property(obj, key) {
            return Ok(Value::Undefined);
        }
        // 访问器描述优先（getter/setter 表命中即访问器属性）。getter/setter
        // 表存函数模板索引（非堆闭包），描述面以占位 NativeFn 暴露函数性
        // （typeof desc.get === "function"）。
        if let Value::Object(r) = obj {
            if let Some(HeapObject::Ordinary {
                getters, setters, ..
            }) = self.heap.get(r.0 as usize)
            {
                let g = getters.get(key).copied();
                let s = setters.get(key).copied();
                if g.is_some() || s.is_some() {
                    let desc = self.alloc_ordinary();
                    if let Some(gf) = g {
                        let placeholder = self.alloc_native_fn("accessor.getter");
                        self.set_native_fn_property(
                            placeholder,
                            "_template_index",
                            Value::Number(gf as f64),
                        );
                        let _ = self.set_property(
                            Value::Object(desc),
                            "get",
                            Value::Object(placeholder),
                        );
                    }
                    if let Some(sf) = s {
                        let placeholder = self.alloc_native_fn("accessor.setter");
                        self.set_native_fn_property(
                            placeholder,
                            "_template_index",
                            Value::Number(sf as f64),
                        );
                        let _ = self.set_property(
                            Value::Object(desc),
                            "set",
                            Value::Object(placeholder),
                        );
                    }
                    let _ =
                        self.set_property(Value::Object(desc), "enumerable", Value::Boolean(true));
                    let _ = self.set_property(
                        Value::Object(desc),
                        "configurable",
                        Value::Boolean(true),
                    );
                    return Ok(Value::Object(desc));
                }
            }
        }
        let value = self.get_property(obj, key)?;
        let desc = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(desc), "value", value);
        let _ = self.set_property(Value::Object(desc), "writable", Value::Boolean(true));
        let _ = self.set_property(Value::Object(desc), "enumerable", Value::Boolean(true));
        let _ = self.set_property(Value::Object(desc), "configurable", Value::Boolean(true));
        Ok(Value::Object(desc))
    }

    /// 按描述对象定义属性（`Object.defineProperty` 底层语义）。
    ///
    /// 支持 value/writable（数据属性）与 get/set（访问器属性）；enumerable/
    /// configurable 位无存储（忽略）。目标为 Proxy 时经 trap 派发由调用方处理。
    pub(crate) fn ordinary_define_property(
        &mut self,
        obj: Value,
        key: &str,
        desc: Value,
    ) -> Result<(), VmError> {
        let get_v = |vm: &mut Vm, k: &str| -> Result<Value, VmError> { vm.get_property(desc, k) };
        let has_get = self.has_property(desc, "get") && {
            let g = get_v(self, "get")?;
            !matches!(g, Value::Undefined)
        };
        let has_set = self.has_property(desc, "set") && {
            let s = get_v(self, "set")?;
            !matches!(s, Value::Undefined)
        };
        if has_get || has_set {
            // 访问器属性：注册 getter/setter 到 Ordinary 访问器表
            // （表存函数模板索引；函数值经 func_template_index_of 解析）
            let Value::Object(r) = obj else {
                return Ok(());
            };
            let idx = r.0 as usize;
            let g_idx = if has_get {
                let g = get_v(self, "get")?;
                self.func_template_index_of(g)
            } else {
                None
            };
            let s_idx = if has_set {
                let s = get_v(self, "set")?;
                self.func_template_index_of(s)
            } else {
                None
            };
            if let Some(HeapObject::Ordinary {
                getters,
                setters,
                has_accessors,
                ..
            }) = self.heap.get_mut(idx)
            {
                if let Some(g) = g_idx {
                    getters.insert(key.to_owned(), g);
                }
                if let Some(s) = s_idx {
                    setters.insert(key.to_owned(), s);
                }
                *has_accessors = 1;
            }
            return Ok(());
        }
        let value = get_v(self, "value")?;
        self.set_property(obj, key, value)
    }

    /// 解析函数值为函数模板索引（Ordinary 访问器表存储形态）。
    ///
    /// 闭包直接取模板索引；访问器描述占位 NativeFn 经 `_template_index`
    /// 属性还原；其余形态（原生函数等）不可注册，返回 `None` 忽略。
    pub(crate) fn func_template_index_of(&self, val: Value) -> Option<usize> {
        match val {
            Value::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::Closure { func_idx, .. }) => Some(*func_idx),
                Some(HeapObject::NativeFn { .. }) => {
                    match self.get_native_fn_property(r, "_template_index") {
                        Some(Value::Number(n)) => Some(n as usize),
                        _ => None,
                    }
                }
                // 裸函数模板句柄（heap index == 模板索引的历史形态）
                _ if (r.0 as usize) < self.module_functions.len() => Some(r.0 as usize),
                _ => None,
            },
            _ => None,
        }
    }

    /// 检查 l instanceof r（沿着 l 的原型链查找 r.prototype）。
    pub fn check_instanceof(&mut self, l: Value, r: Value) -> bool {
        // RegExp 实例（无原型链字段的堆形态）对 RegExp 构造器特判
        if let (Value::Object(lr), Value::Object(rr)) = (l, r) {
            if self.regexp_ctor == Some(rr)
                && matches!(
                    self.heap.get(lr.0 as usize),
                    Some(HeapObject::RegExp { .. })
                )
            {
                return true;
            }
        }
        let target_proto = match self.get_property(r, "prototype") {
            Ok(Value::Object(p)) => p,
            _ => return false,
        };
        // 原型链遍历对 Proxy 感知：链上 Proxy 经 getPrototypeOf trap 解析
        let mut cur = match l {
            Value::Object(lr) if self.proxy_parts(lr).is_some() => {
                match self.proxy_get_prototype_of(lr) {
                    Ok(Value::Object(p)) => Some(p),
                    _ => None,
                }
            }
            other => self.get_prototype(other),
        };
        let mut depth = 0;
        while let Some(proto_ref) = cur {
            if depth > 100 {
                break;
            }
            depth += 1;
            if proto_ref == target_proto {
                return true;
            }
            cur = self.get_prototype(Value::Object(proto_ref));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;

    fn vm() -> Vm {
        Vm::new(0)
    }

    fn obj(vm: &mut Vm) -> Value {
        Value::Object(vm.alloc_ordinary())
    }

    /// 小对象（< 字典阈值）保持在快速 shape 模式，且结构相同的对象共享同一 shape。
    #[test]
    fn small_objects_stay_in_fast_shape_mode_and_share_shape() {
        let mut vm = vm();
        let a = obj(&mut vm);
        let b = obj(&mut vm);
        let _ = vm.set_property(a, "x", Value::Number(1.0));
        let _ = vm.set_property(a, "y", Value::Number(2.0));
        let _ = vm.set_property(b, "x", Value::Number(3.0));
        let _ = vm.set_property(b, "y", Value::Number(4.0));
        let (Value::Object(ra), Value::Object(rb)) = (a, b) else {
            unreachable!()
        };
        let (HeapObject::Ordinary { props: pa, .. }, HeapObject::Ordinary { props: pb, .. }) =
            (&vm.heap[ra.0 as usize], &vm.heap[rb.0 as usize])
        else {
            unreachable!()
        };
        let (OrdinaryProps::Shape { shape: sa, .. }, OrdinaryProps::Shape { shape: sb, .. }) =
            (pa, pb)
        else {
            panic!("小对象必须保持在快速 shape 模式");
        };
        assert_eq!(sa, sb, "结构相同的对象必须共享同一 shape（PIC 命中前提）");
        // 读写经 shape 路径正确
        assert!(matches!(vm.get_property(a, "y"), Ok(Value::Number(2.0))));
        assert!(matches!(vm.get_property(b, "y"), Ok(Value::Number(4.0))));
    }

    /// 属性数达字典阈值后整体转字典模式：海量键对象（Buffer 数值下标等）
    /// 不得沿 shape transition 树逐键克隆前缀（O(n²) 内存爆炸回归防护）。
    #[test]
    fn oversized_objects_switch_to_dict_mode_bounded_memory() {
        let mut vm = vm();
        let o = obj(&mut vm);
        const N: usize = 256; // 远超市面字典阈值 32
        // 引导期（Vm::new 预建原型链）已产生基线 shape，只断言增量
        let shapes_before = vm.shape_table.len();
        for i in 0..N {
            let _ = vm.set_property(o, &i.to_string(), Value::Number(i as f64));
        }
        let Value::Object(r) = o else { unreachable!() };
        let HeapObject::Ordinary { props, .. } = &vm.heap[r.0 as usize] else {
            unreachable!()
        };
        assert!(
            matches!(props, OrdinaryProps::Dict { .. }),
            "超阈值对象必须转字典模式"
        );
        // shape 树只应新增阈值内的过渡（≪ N），证明未逐键克隆前缀
        let shape_growth = vm.shape_table.len() - shapes_before;
        assert!(
            shape_growth < 64,
            "shape 树不应随字典模式对象增长（新增 {shape_growth}）"
        );
        // 全部属性可读、可枚举、可删
        for i in 0..N {
            assert!(matches!(
                vm.get_property(o, &i.to_string()),
                Ok(Value::Number(n)) if n == i as f64
            ));
        }
        assert_eq!(vm.own_entries(r.0 as usize).len(), N);
        vm.delete_property(o, "128");
        assert!(matches!(vm.get_property(o, "128"), Ok(Value::Undefined)));
        assert_eq!(vm.own_entries(r.0 as usize).len(), N - 1);
        // 删除后重写恢复
        let _ = vm.set_property(o, "128", Value::Number(128.0));
        assert!(matches!(
            vm.get_property(o, "128"),
            Ok(Value::Number(128.0))
        ));
    }
}
