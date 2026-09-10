//! 结构化克隆（M5.1）：`worker_threads` 跨线程传值的规范级克隆与序列化。
//!
//! 对齐 HTML StructuredSerialize 的 Node.js 22 LTS 实测子集：
//! - 基本类型 / 字符串 / bigint / Date / RegExp / 数组 / 普通对象 / Map / Set
//!   / ArrayBuffer / TypedArray / DataView；对象图**循环引用**经引用表还原；
//!   克隆后原型简化为普通对象（Node 同）；
//! - 不可克隆值（函数 / Symbol / Promise / WeakMap / Proxy 等）抛
//!   `DataCloneError`（对齐 Node：`postMessage` 同步抛）；
//! - transfer list：ArrayBuffer（或其 TypedArray/DataView 视图）整缓冲移交
//!   并把源缓冲 detach（`byteLength` 归零、视图访问抛 TypeError）；二次
//!   transfer / `markAsUntransferable` 登记过的缓冲抛
//!   `Cannot transfer object of unsupported type.`；
//! - 传输格式：magic `ALSC1` + tag 流（见 [`T_*`] 常量）；跨线程通道保持
//!   String 形态——载荷经 base64 承载（`encode`/`decode` 成对）。
//!
//! 简化口径（登记）：多视图共享同一 ArrayBuffer 的对象图，克隆时各视图按
//! 自身字节区间独立复制（共享关系不保留——V8 保留共享，差异登记）；Map 键
//! 在引擎内本已字符串化（heap Map 存储约束），克隆保持该形态。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashSet;

// `markAsUntransferable(buf)` 登记（线程局部句柄集）：登记过的 ArrayBuffer
// 出现在 transfer list 时抛 DataCloneError（Node 语义）。
thread_local! {
    static UNTRANSFERABLE: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
}

/// `worker_threads.markAsUntransferable` 登记。
pub(crate) fn mark_untransferable(r: aluka_core::ObjectRef) {
    UNTRANSFERABLE.with(|c| c.borrow_mut().insert(r.0));
}

fn is_marked(r: u32) -> bool {
    UNTRANSFERABLE.with(|c| c.borrow().contains(&r))
}

/// 序列化格式魔数（首 5 字节）。
const MAGIC: &[u8; 5] = b"ALSC1";
/// tag 常量（字节流协议）。
const T_UNDEF: u8 = 0;
const T_NULL: u8 = 1;
const T_FALSE: u8 = 2;
const T_TRUE: u8 = 3;
const T_NUM: u8 = 4;
const T_STR: u8 = 5;
const T_BIGINT: u8 = 6;
const T_DATE: u8 = 7;
const T_REGEXP: u8 = 8;
const T_ARRAY: u8 = 9;
const T_OBJECT: u8 = 10;
const T_MAP: u8 = 11;
const T_SET: u8 = 12;
const T_AB: u8 = 13;
const T_TA: u8 = 14;
const T_DV: u8 = 15;
const T_REF: u8 = 16;

/// DataCloneError（DOMException 形态：`name` 供对拍，message 近似 Node）。
pub(crate) fn data_clone_error(vm: &mut Vm, msg: &str) -> VmError {
    let obj = vm.alloc_ordinary();
    let n = vm.alloc_string("DataCloneError".to_owned());
    let _ = vm.set_property(Value::Object(obj), "name", Value::Object(n));
    let m = vm.alloc_string(msg.to_owned());
    let _ = vm.set_property(Value::Object(obj), "message", Value::Object(m));
    let _ = vm.set_property(Value::Object(obj), "code", Value::Number(25.0));
    VmError::Thrown(Value::Object(obj))
}

/// 把 JS 值结构化克隆为自描述字节（不可克隆值抛 DataCloneError）。
///
/// `transfer` 为 transfer list 元素（ArrayBuffer 或其视图）；成功返回后
/// 源缓冲已 detach。错误时不执行任何 detach（Node 语义）。
pub(crate) fn serialize(vm: &mut Vm, root: Value, transfer: &[Value]) -> Result<Vec<u8>, VmError> {
    let mut ser = Ser {
        vm,
        out: Vec::new(),
        objects: Vec::new(),
        transfer: HashSet::new(),
        detach_after: Vec::new(),
    };
    // transfer list：仅接受 ArrayBuffer / TypedArray / DataView（取其底层
    // buffer）；其余（含 MessagePort 本轮登记）抛 unsupported。
    for t in transfer {
        let buf = match t {
            Value::Object(r) => match ser.vm.heap.get(r.0 as usize) {
                Some(HeapObject::ArrayBuffer { .. }) => Some(*r),
                Some(HeapObject::TypedArray { buffer, .. }) => Some(*buffer),
                Some(HeapObject::DataView { buffer, .. }) => Some(*buffer),
                _ => None,
            },
            _ => None,
        };
        let Some(buf_ref) = buf else {
            return Err(data_clone_error(
                ser.vm,
                "Cannot transfer object of unsupported type.",
            ));
        };
        // 已 detach、已 markAsUntransferable 或已登记过 → Node 同文本报错。
        let already = match ser.vm.heap.get(buf_ref.0 as usize) {
            Some(HeapObject::ArrayBuffer { detached, .. }) => *detached || is_marked(buf_ref.0),
            _ => true,
        };
        if already || !ser.transfer.insert(buf_ref.0) {
            return Err(data_clone_error(
                ser.vm,
                "Cannot transfer object of unsupported type.",
            ));
        }
    }
    ser.serialize_value(root)?;
    // 序列化成功：统一 detach 被转移的缓冲（data 清空 + detached 置位；
    // 引用该缓冲的 TypedArray 视图 length 归零——Node 实测 detach 后
    // ta.length/byteLength 为 0、元素读 undefined；DataView 的属性读取抛
    // TypeError 由 property.rs 合成面按 detached 置位处理）。
    for r in &ser.detach_after {
        if let Some(HeapObject::ArrayBuffer { data, detached, .. }) =
            ser.vm.heap.get_mut(*r as usize)
        {
            data.clear();
            *detached = true;
        }
        for obj in ser.vm.heap.iter_mut() {
            if let HeapObject::TypedArray { buffer, length, .. } = obj {
                if buffer.0 == *r {
                    *length = 0;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(ser.out.len() + 5);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&ser.out);
    Ok(out)
}

struct Ser<'a> {
    vm: &'a mut Vm,
    out: Vec<u8>,
    /// 已登记对象句柄（下标即引用索引；所有 Object 统一登记以保共享/循环）
    objects: Vec<u32>,
    /// transfer 目标 buffer 句柄集
    transfer: HashSet<u32>,
    /// 序列化成功后待 detach 的 buffer 句柄（登记于转移发生时）
    detach_after: Vec<u32>,
}

impl Ser<'_> {
    fn tag(&mut self, t: u8) {
        self.out.push(t);
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
    }

    fn f64(&mut self, v: f64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn is_date(&mut self, idx: usize) -> Option<f64> {
        match self.vm.own_value(idx, "_isDate") {
            Some(Value::Boolean(true)) => match self.vm.own_value(idx, "_timeValue") {
                Some(Value::Number(n)) => Some(n),
                _ => Some(f64::NAN),
            },
            _ => None,
        }
    }

    fn serialize_value(&mut self, v: Value) -> Result<(), VmError> {
        match v {
            Value::Undefined => {
                self.tag(T_UNDEF);
                Ok(())
            }
            Value::Null => {
                self.tag(T_NULL);
                Ok(())
            }
            Value::Boolean(b) => {
                self.tag(if b { T_TRUE } else { T_FALSE });
                Ok(())
            }
            Value::Number(n) => {
                self.tag(T_NUM);
                self.f64(n);
                Ok(())
            }
            Value::Object(r) => {
                let idx = r.0 as usize;
                // 引用表命中（循环 / 共享同一对象）→ 回写索引
                if let Some(pos) = self.objects.iter().position(|&o| o == r.0) {
                    self.tag(T_REF);
                    self.u32(pos as u32);
                    return Ok(());
                }
                let kind = match self.vm.heap.get(idx) {
                    Some(HeapObject::String(s)) => {
                        let text = s.clone();
                        self.tag(T_STR);
                        self.str(&text);
                        return Ok(());
                    }
                    Some(HeapObject::BigInt(d)) => {
                        let digits = d.clone();
                        self.tag(T_BIGINT);
                        self.str(&digits);
                        return Ok(());
                    }
                    Some(HeapObject::Closure { .. })
                    | Some(HeapObject::NativeFn { .. })
                    | Some(HeapObject::NativeCtor { .. }) => {
                        return Err(data_clone_error(self.vm, "Function could not be cloned."));
                    }
                    Some(HeapObject::Symbol { .. }) => {
                        return Err(data_clone_error(self.vm, "Symbol could not be cloned."));
                    }
                    Some(HeapObject::RegExp { pattern, flags }) => {
                        self.objects.push(r.0);
                        let (pattern, flags) = (pattern.clone(), flags.clone());
                        self.tag(T_REGEXP);
                        self.str(&pattern);
                        self.str(&flags);
                        return Ok(());
                    }
                    Some(HeapObject::Array { .. }) => "array",
                    Some(HeapObject::Ordinary { .. }) => {
                        if let Some(t) = self.is_date(idx) {
                            self.objects.push(r.0);
                            self.tag(T_DATE);
                            self.i64(t as i64);
                            return Ok(());
                        }
                        "obj"
                    }
                    Some(HeapObject::Map { .. }) => {
                        if self.vm.is_set_instance(v) {
                            "set"
                        } else if self.vm.is_map_instance(v) {
                            "map"
                        } else {
                            return Err(data_clone_error(self.vm, "Object could not be cloned."));
                        }
                    }
                    Some(HeapObject::TypedArray { .. }) => "ta",
                    Some(HeapObject::DataView { .. }) => "dv",
                    Some(HeapObject::ArrayBuffer { .. }) => "ab",
                    Some(HeapObject::Proxy { .. }) | Some(HeapObject::Promise { .. }) => {
                        return Err(data_clone_error(self.vm, "Object could not be cloned."));
                    }
                    _ => {
                        return Err(data_clone_error(self.vm, "Object could not be cloned."));
                    }
                };
                match kind {
                    "array" => self.serialize_array(r.0),
                    "obj" => self.serialize_object(r.0),
                    "map" => self.serialize_map(r.0, false),
                    "set" => self.serialize_map(r.0, true),
                    "ta" => self.serialize_ta(r.0),
                    "dv" => self.serialize_dv(r.0),
                    "ab" => self.serialize_ab(r.0),
                    _ => unreachable!(),
                }
            }
        }
    }

    fn serialize_array(&mut self, r: u32) -> Result<(), VmError> {
        // 先登记（循环引用经骨架回填）再写元素
        self.objects.push(r);
        let elements = match self.vm.heap.get(r as usize) {
            Some(HeapObject::Array { elements, .. }) => elements.clone(),
            _ => return Ok(()),
        };
        self.tag(T_ARRAY);
        self.u32(elements.len() as u32);
        for e in elements {
            self.serialize_value(e)?;
        }
        Ok(())
    }

    fn serialize_object(&mut self, r: u32) -> Result<(), VmError> {
        // 先登记（循环引用经骨架回填）再写键值对
        self.objects.push(r);
        let pairs = self.vm.own_entries(r as usize);
        self.tag(T_OBJECT);
        self.u32(pairs.len() as u32);
        for (k, val) in pairs {
            self.str(&k);
            self.serialize_value(val)?;
        }
        Ok(())
    }

    fn serialize_map(&mut self, r: u32, as_set: bool) -> Result<(), VmError> {
        self.objects.push(r);
        let entries = match self.vm.heap.get(r as usize) {
            Some(HeapObject::Map { entries }) => entries.clone(),
            _ => return Ok(()),
        };
        self.tag(if as_set { T_SET } else { T_MAP });
        self.u32(entries.len() as u32);
        for (k, val) in entries {
            if as_set {
                self.serialize_value(val)?;
            } else {
                // 键为原始 `Value`（数字/布尔/对象等），按值序列化——
                // 与读端 `self.value()` 成对，改一端会让跨线程克隆静默错值
                self.serialize_value(k)?;
                self.serialize_value(val)?;
            }
        }
        Ok(())
    }

    /// ArrayBuffer：转移（transfer 集命中）或克隆都携带全量字节；
    /// 转移登记 detach，克隆保持源不动。
    fn serialize_ab(&mut self, r: u32) -> Result<(), VmError> {
        self.objects.push(r);
        let (data, detached) = match self.vm.heap.get(r as usize) {
            Some(HeapObject::ArrayBuffer { data, detached, .. }) => (data.clone(), *detached),
            _ => return Ok(()),
        };
        if detached {
            return Err(data_clone_error(
                self.vm,
                "ArrayBuffer could not be cloned.",
            ));
        }
        let transfer = self.transfer.contains(&r);
        if transfer {
            self.detach_after.push(r);
        }
        self.tag(T_AB);
        self.u32(data.len() as u32);
        self.out.extend_from_slice(&data);
        Ok(())
    }

    /// TypedArray：克隆 = 复制视图字节区间为新缓冲上的视图；
    /// 其底层 buffer 在 transfer 集 → 整缓冲移交并登记 detach（视图字节
    /// 区间与移交内容一致，收端按 kind/len 还原）。
    fn serialize_ta(&mut self, r: u32) -> Result<(), VmError> {
        self.objects.push(r);
        let info = match self.vm.heap.get(r as usize) {
            Some(HeapObject::TypedArray {
                kind,
                buffer,
                byte_offset,
                length,
            }) => Some((*kind, *buffer, *byte_offset, *length)),
            _ => None,
        };
        let Some((kind, buffer, byte_offset, length)) = info else {
            return Ok(());
        };
        let elem = kind.elem_size();
        let view_end = byte_offset + length * elem;
        let transfer = self.transfer.contains(&buffer.0);
        // 快照 buffer 字节（脱离借用后再写 out）
        let data = match self.vm.heap.get(buffer.0 as usize) {
            Some(HeapObject::ArrayBuffer { data, detached, .. }) => {
                if *detached {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
                if transfer {
                    self.detach_after.push(buffer.0);
                    // 整缓冲移交：offset 之外的字节也携带（视图区间内含）
                    data.clone()
                } else if data.len() >= view_end {
                    data[byte_offset..view_end].to_vec()
                } else {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
            }
            _ => return Ok(()),
        };
        // clone：独立区间缓冲（offset 归零——区间从 0 起自洽）；
        // transfer：整缓冲移交（保留原 byte_offset 指向移交缓冲）。
        let wire_offset = if transfer { byte_offset } else { 0 };
        self.tag(T_TA);
        self.out.push(kind_tag(kind));
        self.u32(wire_offset as u32);
        self.u32(length as u32);
        self.u32(data.len() as u32);
        self.out.extend_from_slice(&data);
        Ok(())
    }

    fn serialize_dv(&mut self, r: u32) -> Result<(), VmError> {
        self.objects.push(r);
        let info = match self.vm.heap.get(r as usize) {
            Some(HeapObject::DataView {
                buffer,
                byte_offset,
                byte_length,
            }) => Some((*buffer, *byte_offset, *byte_length)),
            _ => None,
        };
        let Some((buffer, byte_offset, byte_length)) = info else {
            return Ok(());
        };
        let transfer = self.transfer.contains(&buffer.0);
        let data = match self.vm.heap.get(buffer.0 as usize) {
            Some(HeapObject::ArrayBuffer { data, detached, .. }) => {
                if *detached {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
                if transfer {
                    self.detach_after.push(buffer.0);
                    data.clone()
                } else if data.len() >= byte_offset + byte_length {
                    data[byte_offset..byte_offset + byte_length].to_vec()
                } else {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
            }
            _ => return Ok(()),
        };
        // DV 字节承载：clone = 区间自足（offset 0）；transfer = 整缓冲 +
        // 原 offset 保留。载荷 = offset + 字节数 + 视图长 + 字节（字节数
        // 与视图长分离——transfer 时整缓冲字节 > 视图长）。
        let wire_offset = if transfer { byte_offset } else { 0 };
        self.tag(T_DV);
        self.u32(wire_offset as u32);
        self.u32(data.len() as u32);
        self.u32(byte_length as u32);
        self.out.extend_from_slice(&data);
        Ok(())
    }
}

/// 反序列化：字节 → 本线程 VM 堆上的值（容器引用表支持循环/共享还原）。
pub(crate) fn deserialize(vm: &mut Vm, bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 5 || &bytes[..5] != MAGIC {
        return Err("structured clone: bad magic".to_owned());
    }
    let mut de = De {
        vm,
        buf: &bytes[5..],
        pos: 0,
        objects: Vec::new(),
    };
    de.value()
}

struct De<'a, 'v> {
    vm: &'v mut Vm,
    buf: &'a [u8],
    pos: usize,
    /// 已构建对象（与序列化登记顺序一致）
    objects: Vec<Value>,
}

impl De<'_, '_> {
    fn u8(&mut self) -> Result<u8, String> {
        let b = *self.buf.get(self.pos).ok_or("clone: eof")?;
        self.pos += 1;
        Ok(b)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let s = self.buf.get(self.pos..self.pos + 4).ok_or("clone: eof")?;
        self.pos += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i64(&mut self) -> Result<i64, String> {
        let s = self.buf.get(self.pos..self.pos + 8).ok_or("clone: eof")?;
        self.pos += 8;
        Ok(i64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }

    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.i64()? as u64))
    }

    fn str(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        let s = self.buf.get(self.pos..self.pos + len).ok_or("clone: eof")?;
        self.pos += len;
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn take(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let s = self.buf.get(self.pos..self.pos + n).ok_or("clone: eof")?;
        self.pos += n;
        Ok(s.to_vec())
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.u8()? {
            T_UNDEF => Ok(Value::Undefined),
            T_NULL => Ok(Value::Null),
            T_FALSE => Ok(Value::Boolean(false)),
            T_TRUE => Ok(Value::Boolean(true)),
            T_NUM => Ok(Value::Number(self.f64()?)),
            T_STR => {
                let s = self.str()?;
                Ok(Value::Object(self.vm.alloc_string(s)))
            }
            T_BIGINT => {
                let d = self.str()?;
                Ok(Value::Object(self.vm.alloc_bigint(d)))
            }
            T_DATE => {
                let ms = self.i64()?;
                let t = Value::Number(ms as f64);
                let d = self
                    .vm
                    .construct_date(&[t])
                    .map_err(|_| "clone: date".to_owned())?;
                self.objects.push(d);
                Ok(d)
            }
            T_REGEXP => {
                let pattern = self.str()?;
                let flags = self.str()?;
                let r = self.vm.push_object(HeapObject::RegExp { pattern, flags });
                self.objects.push(Value::Object(r));
                Ok(Value::Object(r))
            }
            T_REF => {
                let idx = self.u32()? as usize;
                self.objects
                    .get(idx)
                    .copied()
                    .ok_or_else(|| format!("clone: bad ref {idx} (table {})", self.objects.len()))
            }
            T_ARRAY => {
                let len = self.u32()? as usize;
                let arr = self.vm.alloc_array(Vec::new());
                self.objects.push(Value::Object(arr));
                for i in 0..len {
                    let v = self.value()?;
                    let _ = self.vm.set_property(Value::Object(arr), &i.to_string(), v);
                }
                Ok(Value::Object(arr))
            }
            T_OBJECT => {
                let count = self.u32()? as usize;
                let obj = self.vm.alloc_ordinary();
                self.objects.push(Value::Object(obj));
                for _ in 0..count {
                    let k = self.str()?;
                    let v = self.value()?;
                    let _ = self.vm.set_property(Value::Object(obj), &k, v);
                }
                Ok(Value::Object(obj))
            }
            T_MAP => {
                let count = self.u32()? as usize;
                let map = self.vm.alloc_map(Vec::new());
                self.objects.push(Value::Object(map));
                for _ in 0..count {
                    let k = self.value()?;
                    let v = self.value()?;
                    self.vm.push_map_entry(map, k, v);
                }
                Ok(Value::Object(map))
            }
            T_SET => {
                let count = self.u32()? as usize;
                let set = self.vm.alloc_map(Vec::new());
                self.vm.register_set_instance(set);
                self.objects.push(Value::Object(set));
                for _ in 0..count {
                    let v = self.value()?;
                    // 键与值同存元素原值（SameValueZero 去重语义）
                    self.vm.push_map_entry(set, v, v);
                }
                Ok(Value::Object(set))
            }
            T_AB => {
                let len = self.u32()? as usize;
                let data = self.take(len)?;
                let ab = self.vm.alloc_array_buffer(data, false, false, 0);
                self.objects.push(Value::Object(ab));
                Ok(Value::Object(ab))
            }
            T_TA => {
                let kind_raw = self.u8()?;
                let byte_offset = self.u32()? as usize;
                let elems = self.u32()? as usize;
                let byte_len = self.u32()? as usize;
                let bytes = self.take(byte_len)?;
                let kind =
                    from_kind_tag(kind_raw).ok_or_else(|| "clone: bad ta kind".to_owned())?;
                // 克隆/转移的视图字节均从自身区间起（ta 原 offset 还原，
                // dv 原样）——新建缓冲承载区间字节。
                let ab = self.vm.alloc_array_buffer(bytes, false, false, 0);
                let ta = self.vm.alloc_typed_array(kind, ab, byte_offset, elems);
                self.objects.push(Value::Object(ta));
                Ok(Value::Object(ta))
            }
            T_DV => {
                let byte_offset = self.u32()? as usize;
                let data_len = self.u32()? as usize;
                let view_len = self.u32()? as usize;
                let bytes = self.take(data_len)?;
                let ab = self.vm.alloc_array_buffer(bytes, false, false, 0);
                let dv = self.vm.alloc_data_view(ab, byte_offset, view_len);
                self.objects.push(Value::Object(dv));
                Ok(Value::Object(dv))
            }
            other => Err(format!("clone: unknown tag {other}")),
        }
    }
}

/// Map/Set 追加条目（键为原始 `Value` + SameValueZero 语义；含写屏障）。
impl Vm {
    pub(crate) fn push_map_entry(&mut self, map: aluka_core::ObjectRef, key: Value, val: Value) {
        // 先在不可变借用下求 SameValueZero 命中下标（比较需读堆判定字符串内容，
        // 与 interpreter 的 Map/Set 方法块同构），再进入可变借用写入
        let hit = match self.heap.get(map.0 as usize) {
            Some(HeapObject::Map { entries }) => entries
                .iter()
                .position(|(k, _)| self.values_same_zero(*k, key)),
            _ => None,
        };
        if let Some(HeapObject::Map { entries }) = self.heap.get_mut(map.0 as usize) {
            if let Some(i) = hit {
                entries[i].1 = val;
            } else {
                entries.push((key, val));
            }
        }
        // 写屏障：键与值都可能是堆对象引用（与 interpreter 的 set/add 分支一致；
        // 此处原实现无屏障，属既有缺口，一并补上）
        self.gc_write_barrier(map, key);
        self.gc_write_barrier(map, val);
    }
}

/// TypedKind → 传输 tag（枚举序稳定，deserialize 反向映射）。
fn kind_tag(kind: crate::typed_array::TypedKind) -> u8 {
    use crate::typed_array::TypedKind::*;
    match kind {
        Int8 => 0,
        Uint8 => 1,
        Uint8Clamped => 2,
        Int16 => 3,
        Uint16 => 4,
        Int32 => 5,
        Uint32 => 6,
        Float32 => 7,
        Float64 => 8,
        BigInt64 => 9,
        BigUint64 => 10,
    }
}

fn from_kind_tag(tag: u8) -> Option<crate::typed_array::TypedKind> {
    use crate::typed_array::TypedKind::*;
    Some(match tag {
        0 => Int8,
        1 => Uint8,
        2 => Uint8Clamped,
        3 => Int16,
        4 => Uint16,
        5 => Int32,
        6 => Uint32,
        7 => Float32,
        8 => Float64,
        9 => BigInt64,
        10 => BigUint64,
        _ => return None,
    })
}
