//! 迭代器协议实现：Array / String / Map / Set

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

// ===== 内建迭代器对象表面（next / Symbol.iterator）=====
//
// 四类内建迭代器（Array/String/Map/Set）是 `Ordinary` 对象 + 标记属性；
// 除 CALL_METHOD 的按名硬编码分派外，这里为每个迭代器对象挂**真实属性**：
// - `next` → NativeFn `"Iterator.prototype.next"`（handler 按标记分流到
//   对应的 `*_iterator_next`）；
// - `Symbol.iterator` → NativeFn `"Iterator.prototype.Symbol.iterator"`
//   （调用返回 receiver 自身——迭代器自迭代，规范语义）。
//
// 没有这两个属性时 `typeof it.next` / `typeof it[Symbol.iterator]` 为
// `undefined`，`[...it]` 静默给出空结果（`[...]` 的物化在
// [`Vm::collect_iter_values`] 按标记识别，不依赖属性）。
//
// **已登记偏离**：Node 把这两个方法挂在各迭代器**原型**上
// （`Object.prototype.hasOwnProperty.call(it, "next") === false`）；本实现
// 迭代器对象没有独立原型面，挂为**自有属性**，因此
// `Object.prototype.hasOwnProperty.call(it, "next") === true`。行为等价
// （可读、可调、自迭代），仅属性归属不同——属可接受折衷，如实登记。

impl Vm {
    /// 为内建迭代器对象挂 `next` / `Symbol.iterator` 真实属性（偏离见上方登记）。
    fn attach_iterator_surface(&mut self, obj: ObjectRef) {
        let next = self.alloc_native_fn("Iterator.prototype.next");
        let _ = self.set_property(Value::Object(obj), "next", Value::Object(next));
        if let Value::Object(sym_ref) = self.well_known_symbol("iterator") {
            let key = crate::symbol::mangled_key(sym_ref);
            let f = self.alloc_native_fn("Iterator.prototype.Symbol.iterator");
            let _ = self.set_property(Value::Object(obj), &key, Value::Object(f));
        }
    }
}

// ===== Array Iterator =====

thread_local! { static ARRAY_ITER_POS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new()); }

impl Vm {
    pub(crate) fn is_array_iterator(&self, val: Value) -> bool {
        matches!(val, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isArrayIterator"))
    }

    pub(crate) fn alloc_array_iterator_kind(&mut self, arr: ObjectRef, kind: &str) -> Value {
        let obj = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(obj), "_isArrayIterator", Value::Boolean(true));
        let _ = self.set_property(Value::Object(obj), "_iterArray", Value::Object(arr));
        if kind != "values" {
            let flag = self.alloc_string(kind.to_owned());
            let _ = self.set_property(Value::Object(obj), "_iterKind", Value::Object(flag));
        }
        ARRAY_ITER_POS.with(|c| c.borrow_mut().insert(obj.0, 0));
        self.attach_iterator_surface(obj);
        Value::Object(obj)
    }

    pub(crate) fn alloc_array_iterator(&mut self, arr: ObjectRef) -> Value {
        self.alloc_array_iterator_kind(arr, "values")
    }

    pub(crate) fn array_iterator_next(&mut self, iter: ObjectRef) -> Result<Value, VmError> {
        let arr = match self.heap.get(iter.0 as usize) {
            Some(HeapObject::Ordinary { .. }) => {
                match self.own_value(iter.0 as usize, "_iterArray") {
                    Some(Value::Object(a)) => a,
                    _ => return self.make_iterator_result(Value::Undefined, true),
                }
            }
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let kind = iter_kind(self, iter);
        let pos = ARRAY_ITER_POS.with(|c| c.borrow().get(&iter.0).copied().unwrap_or(0));
        let result = match self.heap.get(arr.0 as usize) {
            Some(HeapObject::Array { elements, .. }) if pos < elements.len() => {
                match kind.as_str() {
                    "keys" => self.make_iterator_result(Value::Number(pos as f64), false)?,
                    "entries" => {
                        let pair = self.alloc_array(vec![Value::Number(pos as f64), elements[pos]]);
                        self.make_iterator_result(Value::Object(pair), false)?
                    }
                    _ => self.make_iterator_result(elements[pos], false)?,
                }
            }
            _ => self.make_iterator_result(Value::Undefined, true)?,
        };
        ARRAY_ITER_POS.with(|c| c.borrow_mut().insert(iter.0, pos + 1));
        Ok(result)
    }
}

// ===== String Iterator =====

thread_local! { static STRING_ITER_POS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new()); }

impl Vm {
    pub(crate) fn is_string_iterator(&self, val: Value) -> bool {
        matches!(val, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isStrIterator"))
    }

    pub(crate) fn alloc_string_iterator(&mut self, str_ref: ObjectRef) -> Value {
        let obj = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(obj), "_isStrIterator", Value::Boolean(true));
        let _ = self.set_property(Value::Object(obj), "_iterStr", Value::Object(str_ref));
        STRING_ITER_POS.with(|c| c.borrow_mut().insert(obj.0, 0));
        self.attach_iterator_surface(obj);
        Value::Object(obj)
    }

    pub(crate) fn string_iterator_next(&mut self, iter: ObjectRef) -> Result<Value, VmError> {
        let str_ref = match self.heap.get(iter.0 as usize) {
            Some(HeapObject::Ordinary { .. }) => {
                match self.own_value(iter.0 as usize, "_iterStr") {
                    Some(Value::Object(s)) => s,
                    _ => return self.make_iterator_result(Value::Undefined, true),
                }
            }
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let text = match self.heap.get(str_ref.0 as usize) {
            Some(HeapObject::String(s)) => s.clone(),
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        // JS 字符串迭代按 Unicode 码点推进（代理对为单一产出）
        let chars: Vec<char> = text.chars().collect();
        let pos = STRING_ITER_POS.with(|c| c.borrow().get(&iter.0).copied().unwrap_or(0));
        let result = if pos < chars.len() {
            let mut buf = [0u8; 4];
            let s = chars[pos].encode_utf8(&mut buf).to_string();
            let sv = Value::Object(self.alloc_string(s));
            self.make_iterator_result(sv, false)?
        } else {
            self.make_iterator_result(Value::Undefined, true)?
        };
        STRING_ITER_POS.with(|c| c.borrow_mut().insert(iter.0, pos + 1));
        Ok(result)
    }
}

// ===== Map / Set 实例识别 =====
//
// Map 与 Set 共用 `HeapObject::Map` 变体（见 heap.rs）；区分二者靠构造时
// 登记的句柄集合（`new Set()` 在 call.rs 登记）。句柄在线程局部堆内唯一，
// 登记表只增不减——与迭代位置表同生命周期模型，避免 GC 追溯复杂度。

thread_local! { static SET_HANDLES: RefCell<HashSet<u32>> = RefCell::new(HashSet::new()); }

impl Vm {
    /// 登记 `new Set()` 产出的句柄（构造路径调用）。
    pub(crate) fn register_set_instance(&mut self, r: ObjectRef) {
        SET_HANDLES.with(|c| c.borrow_mut().insert(r.0));
    }

    /// 值是否为 Map 实例（`HeapObject::Map` 变体且非 Set 登记）。
    pub(crate) fn is_map_instance(&self, val: Value) -> bool {
        matches!(val, Value::Object(r)
            if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Map { .. }))
                && !SET_HANDLES.with(|c| c.borrow().contains(&r.0)))
    }

    /// 值是否为 Set 实例。
    pub(crate) fn is_set_instance(&self, val: Value) -> bool {
        matches!(val, Value::Object(r)
            if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Map { .. }))
                && SET_HANDLES.with(|c| c.borrow().contains(&r.0)))
    }
}

// ===== Map Iterator =====

thread_local! { static MAP_ITER_POS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new()); }

impl Vm {
    pub(crate) fn is_map_iterator(&self, val: Value) -> bool {
        matches!(val, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isMapIterator"))
    }

    pub(crate) fn alloc_map_iterator(&mut self, map_ref: ObjectRef, kind: &str) -> Value {
        let obj = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(obj), "_isMapIterator", Value::Boolean(true));
        let _ = self.set_property(Value::Object(obj), "_iterMap", Value::Object(map_ref));
        if kind != "entries" {
            let flag = self.alloc_string(kind.to_owned());
            let _ = self.set_property(Value::Object(obj), "_iterKind", Value::Object(flag));
        }
        MAP_ITER_POS.with(|c| c.borrow_mut().insert(obj.0, 0));
        self.attach_iterator_surface(obj);
        Value::Object(obj)
    }

    /// Map 迭代：kind=entries 产出 `[key, value]`、keys 产出 key、values 产出 value。
    pub(crate) fn map_iterator_next(&mut self, iter: ObjectRef) -> Result<Value, VmError> {
        let map_ref = match self.heap.get(iter.0 as usize) {
            Some(HeapObject::Ordinary { .. }) => {
                match self.own_value(iter.0 as usize, "_iterMap") {
                    Some(Value::Object(m)) => m,
                    _ => return self.make_iterator_result(Value::Undefined, true),
                }
            }
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let kind = iter_kind(self, iter);
        // 有序项快照（键为原始 Value——SameValueZero 语义，见 heap.rs）
        let entries: Vec<(Value, Value)> = match self.heap.get(map_ref.0 as usize) {
            Some(HeapObject::Map { entries }) => entries.clone(),
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let pos = MAP_ITER_POS.with(|c| c.borrow().get(&iter.0).copied().unwrap_or(0));
        let result = if pos < entries.len() {
            let (k, v) = &entries[pos];
            // 键身份：直接产出原键 Value（对象键必须保持同一，不得重建字符串）
            let kv = *k;
            match kind.as_str() {
                "keys" => self.make_iterator_result(kv, false)?,
                "values" => self.make_iterator_result(*v, false)?,
                _ => {
                    let pair = self.alloc_array(vec![kv, *v]);
                    self.make_iterator_result(Value::Object(pair), false)?
                }
            }
        } else {
            self.make_iterator_result(Value::Undefined, true)?
        };
        MAP_ITER_POS.with(|c| c.borrow_mut().insert(iter.0, pos + 1));
        Ok(result)
    }
}

// ===== Set Iterator =====

thread_local! { static SET_ITER_POS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new()); }

impl Vm {
    pub(crate) fn is_set_iterator(&self, val: Value) -> bool {
        matches!(val, Value::Object(r) if self.has_own_slot(r.0 as usize, "_isSetIterator"))
    }

    pub(crate) fn alloc_set_iterator(&mut self, set_ref: ObjectRef, kind: &str) -> Value {
        let obj = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(obj), "_isSetIterator", Value::Boolean(true));
        let _ = self.set_property(Value::Object(obj), "_iterSet", Value::Object(set_ref));
        if kind != "values" {
            let flag = self.alloc_string(kind.to_owned());
            let _ = self.set_property(Value::Object(obj), "_iterKind", Value::Object(flag));
        }
        SET_ITER_POS.with(|c| c.borrow_mut().insert(obj.0, 0));
        self.attach_iterator_surface(obj);
        Value::Object(obj)
    }

    /// Set 迭代：kind=values 产出元素、entries 产出 `[v, v]`、keys 产出元素（别名）。
    pub(crate) fn set_iterator_next(&mut self, iter: ObjectRef) -> Result<Value, VmError> {
        let set_ref = match self.heap.get(iter.0 as usize) {
            Some(HeapObject::Ordinary { .. }) => {
                match self.own_value(iter.0 as usize, "_iterSet") {
                    Some(Value::Object(s)) => s,
                    _ => return self.make_iterator_result(Value::Undefined, true),
                }
            }
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let kind = iter_kind(self, iter);
        // Set 的 value 字段保留元素原值（构造时 key=字符串化、value=原值）
        let values: Vec<Value> = match self.heap.get(set_ref.0 as usize) {
            Some(HeapObject::Map { entries }) => entries.iter().map(|(_, v)| *v).collect(),
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let pos = SET_ITER_POS.with(|c| c.borrow().get(&iter.0).copied().unwrap_or(0));
        let result = if pos < values.len() {
            let v = values[pos];
            match kind.as_str() {
                "entries" => {
                    let pair = self.alloc_array(vec![v, v]);
                    self.make_iterator_result(Value::Object(pair), false)?
                }
                _ => self.make_iterator_result(v, false)?,
            }
        } else {
            self.make_iterator_result(Value::Undefined, true)?
        };
        SET_ITER_POS.with(|c| c.borrow_mut().insert(iter.0, pos + 1));
        Ok(result)
    }
}

// ===== Shared helpers =====

/// 读取迭代器对象的 kind（缺省按调用方类型传入的默认值；此处统一读
/// `_iterKind` 字符串属性，无则返回调用方兜底值）。
fn iter_kind(vm: &Vm, iter: ObjectRef) -> String {
    match vm.own_value(iter.0 as usize, "_iterKind") {
        Some(Value::Object(k)) => match vm.heap.get(k.0 as usize) {
            Some(HeapObject::String(text)) => text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

impl Vm {
    fn make_iterator_result(&mut self, value: Value, done: bool) -> Result<Value, VmError> {
        let result = self.alloc_ordinary();
        self.set_property(Value::Object(result), "value", value)?;
        self.set_property(Value::Object(result), "done", Value::Boolean(done))?;
        Ok(Value::Object(result))
    }

    /// 排空一个内建迭代器对象（`[...it]` / `Array.from(it)` 用），返回全部产出值。
    ///
    /// 会**消耗**迭代器（与 Node 一致：迭代器对象是"一次性"的，重复展开第二次为空）。
    /// 逐次调用对应的 `*_iterator_next` 并从结果对象的 `done`/`value` 属性取值。
    fn drain_iterator_to_values(&mut self, it: Value) -> Result<Vec<Value>, VmError> {
        let mut out = Vec::new();
        let Value::Object(obj) = it else {
            return Ok(out);
        };
        loop {
            let r = if self.is_array_iterator(it) {
                self.array_iterator_next(obj)?
            } else if self.is_string_iterator(it) {
                self.string_iterator_next(obj)?
            } else if self.is_map_iterator(it) {
                self.map_iterator_next(obj)?
            } else if self.is_set_iterator(it) {
                self.set_iterator_next(obj)?
            } else {
                break;
            };
            let Value::Object(ro) = r else { break };
            if matches!(
                self.own_value(ro.0 as usize, "done"),
                Some(Value::Boolean(true))
            ) {
                break;
            }
            out.push(
                self.own_value(ro.0 as usize, "value")
                    .unwrap_or(Value::Undefined),
            );
        }
        Ok(out)
    }

    /// 物化可迭代值（`[...x]` / `f(...args)` 展开语义）的全部产出元素。
    ///
    /// 覆盖内建可迭代类型：Array（元素直取）、String（逐码点）、Map
    /// （entries 对 `[key, value]`）、Set（元素）、TypedArray（快照）、
    /// **四类内建迭代器对象**（排空——此前落入空分支，`[...it]` 静默为 `[]`）。
    /// 自定义可迭代对象（Symbol.iterator 方法）不在本路径——走
    /// GetIterator 协议（见 interpreter `Op::GetIterator`）。
    pub(crate) fn collect_iter_values(&mut self, val: Value) -> Result<Vec<Value>, VmError> {
        // 内建迭代器对象：走迭代协议逐项取（先于类型直取分支，且会消耗迭代器）
        if self.is_array_iterator(val)
            || self.is_string_iterator(val)
            || self.is_map_iterator(val)
            || self.is_set_iterator(val)
        {
            return self.drain_iterator_to_values(val);
        }
        // 先快照源数据（避免堆借用跨可变分配）
        enum Src {
            Array(Vec<Value>),
            Entries(Vec<(Value, Value)>, bool), // (entries, is_set)
            Text(String),
        }
        let src = if let Value::Object(r) = val {
            let is_set = self.is_set_instance(val);
            match self.heap.get(r.0 as usize) {
                Some(HeapObject::Array { elements, .. }) => Some(Src::Array(elements.clone())),
                Some(HeapObject::Map { entries }) => Some(Src::Entries(entries.clone(), is_set)),
                Some(HeapObject::String(text)) => Some(Src::Text(text.clone())),
                _ => {
                    if self.is_typed_array(val) {
                        // typed array 单独取快照（ta_to_values 需要 &mut self）
                        None
                    } else {
                        None
                    }
                }
            }
        } else {
            None
        };
        let mut out: Vec<Value> = Vec::new();
        match src {
            Some(Src::Array(elements)) => out = elements,
            Some(Src::Entries(entries, is_set)) => {
                if is_set {
                    // Set：产出元素原值（有序）
                    out = entries.iter().map(|(_, v)| *v).collect();
                } else {
                    // Map：产出 [key, value] 对（键为原始 Value，保持键身份）
                    for (k, v) in entries {
                        let pair = self.alloc_array(vec![k, v]);
                        out.push(Value::Object(pair));
                    }
                }
            }
            Some(Src::Text(text)) => {
                for c in text.chars() {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf).to_string();
                    out.push(Value::Object(self.alloc_string(s)));
                }
            }
            None => {
                if let Value::Object(r) = val
                    && self.is_typed_array(val)
                {
                    out = self.ta_to_values(r)?;
                }
            }
        }
        Ok(out)
    }
}
