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
//! tag 登记（Node 22 实测语义；[17..19] 为 M5.1 语义收口新增）：
//! - `T_AB`(13) ArrayBuffer；`T_SAB`(17) SharedArrayBuffer（载荷与 `T_AB`
//!   相同，克隆后仍为 SAB——Node 实测 `structuredClone(sab) instanceof
//!   SharedArrayBuffer` 为 true）；
//! - `T_DATE`(7) 有效 Date（i64 毫秒）；`T_DATE_INVALID`(18) Invalid Date
//!   （`new Date(NaN)` 克隆后 `getTime()` 仍为 NaN）；
//! - `T_ERROR`(19) Error 实例：`name`(str) + `message`(str) + flags(u8)；
//!   `flags` bit0 表示携带 `stack`(str)、bit1 表示携带 `cause`(value)。`cause`
//!   递归序列化（deep copy；cause 为函数则 DataCloneError——Node 实测）；
//!   反序列化重建 `error_prototype` 实例（`instanceof Error` 为真）并使
//!   `message`/`name`/`stack`/`cause` 不可枚举（Node 克隆体 `Object.keys` 为空集）；
//! - `T_TA`(14)/`T_DV`(15) 载荷首位新增 flags(u8)（bit0 = 底层缓冲为共享
//!   缓冲）：克隆出的视图挂在 SAB 上而非 ArrayBuffer。
//!
//! 简化口径（登记）：多视图共享同一 ArrayBuffer 的对象图，克隆时各视图按
//! 自身字节区间独立复制（共享关系不保留——V8 保留共享，差异登记）；Map 键
//! 在引擎内本已字符串化（heap Map 存储约束），克隆保持该形态。
//!
//! 仍存偏离（登记，非本轮收口范围）：
//! 1. `SharedArrayBuffer` 克隆**只保留 SAB 形态、不共享底层内存**（Node 实测
//!    克隆体与源共享同一内存；跨 Worker 共享见 `typed_array.rs` 顶部登记，
//!    属 M5 未接线能力）；
//! 2. 数据属性与访问器属性的**交错键序**：访问器表为 HashMap（无插入序可
//!    依），克隆按「数据属性在前 + 访问器键升序」输出（Node 按插入序）；
//! 3. Error 克隆体自有属性名集合为 `["message","name"]`（Node 为
//!    `["stack","message"]`——本运行时 Error 无栈实现，源 `e.stack` 本身即
//!    `undefined`，故克隆体 `stack` 同为 undefined）；用户改写过的 `e.name`
//!    在克隆体上沿用改写值（Node 回落构造器名）。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
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
/// SharedArrayBuffer 克隆（载荷与 `T_AB` 相同；SAB 永不可 transfer）
const T_SAB: u8 = 17;
/// Invalid Date（无载荷；有效 Date 走 `T_DATE`）
const T_DATE_INVALID: u8 = 18;
/// Error 实例（name/message/flags/[stack]/[cause]）
const T_ERROR: u8 = 19;

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
    };
    // transfer list：仅接受 ArrayBuffer / TypedArray / DataView（取其底层
    // buffer）；其余（含 MessagePort 本轮登记）抛 unsupported。
    for t in transfer {
        let buf = match t.case() {
            ValueCase::Object(r) => match ser.vm.heap.get(r.0 as usize) {
                Some(HeapObject::ArrayBuffer { .. }) => Some(r),
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
        // SharedArrayBuffer（含挂在 SAB 上的 TypedArray/DataView 视图）不可
        // transfer：Node 实测 `structuredClone(sab, { transfer: [sab] })`
        // 抛 DataCloneError（SAB 只能共享，不能移交）。
        let shared = matches!(
            ser.vm.heap.get(buf_ref.0 as usize),
            Some(HeapObject::ArrayBuffer { shared: true, .. })
        );
        if shared {
            return Err(data_clone_error(
                ser.vm,
                "Cannot transfer object of unsupported type.",
            ));
        }
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
    // 序列化成功：**transfer list 内每个缓冲**一律 detach（Node 实测：未出现
    // 在被克隆值里的 buffer 同样被 detach——源置为 detached；旧实现只处理
    // 「图内命中」的缓冲，属登记偏离）。data 清空 + detached 置位；引用该缓冲
    // 的 TypedArray 视图 length 归零（Node 实测 detach 后 ta.length/byteLength
    // 为 0、元素读 undefined；DataView 的属性读取抛 TypeError 由 property.rs
    // 合成面按 detached 置位处理）。
    let transferred: Vec<u32> = ser.transfer.iter().copied().collect();
    for r in transferred {
        if let Some(HeapObject::ArrayBuffer { data, detached, .. }) =
            ser.vm.heap.get_mut(r as usize)
        {
            data.clear();
            *detached = true;
        }
        for obj in ser.vm.heap.iter_mut() {
            if let HeapObject::TypedArray { buffer, length, .. } = obj {
                if buffer.0 == r {
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
        match self.vm.own_value(idx, "_isDate").map(|v| v.case()) {
            Some(ValueCase::Boolean(true)) => match self.vm.own_value(idx, "_timeValue").map(|v| v.case()) {
                Some(ValueCase::Number(n)) => Some(n),
                _ => Some(f64::NAN),
            },
            _ => None,
        }
    }

    /// Error 实例判定：Ordinary 且原型即 `error_prototype`（Node 实测：克隆
    /// 体 `instanceof Error` 为真；普通对象字面量 `{name:'Error'}` 不参与）。
    fn is_error(&self, idx: usize) -> bool {
        match self.vm.heap.get(idx) {
            Some(HeapObject::Ordinary { proto, .. }) => {
                self.vm.error_prototype.is_some() && *proto == self.vm.error_prototype
            }
            _ => false,
        }
    }

    fn serialize_value(&mut self, v: Value) -> Result<(), VmError> {
        match v.case() {
            Value::Undefined => {
                self.tag(T_UNDEF);
                Ok(())
            }
            Value::Null => {
                self.tag(T_NULL);
                Ok(())
            }
            ValueCase::Boolean(b) => {
                self.tag(if b { T_TRUE } else { T_FALSE });
                Ok(())
            }
            ValueCase::Number(n) => {
                self.tag(T_NUM);
                self.f64(n);
                Ok(())
            }
            ValueCase::Object(r) => {
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
                            // Invalid Date（Node 实测 `structuredClone(new Date(NaN))`
                            // 仍是 Invalid Date）：i64 载荷无法承载 NaN（旧实现截断
                            // 成 0 → 变 1970-01-01），单开标记承载。
                            if t.is_finite() {
                                self.tag(T_DATE);
                                self.i64(t as i64);
                            } else {
                                self.tag(T_DATE_INVALID);
                            }
                            return Ok(());
                        }
                        // Error 实例（原型 === error_prototype）：走专用标记保
                        // `instanceof Error` 与 name/message/cause（Node 实测）。
                        if self.is_error(idx) { "error" } else { "obj" }
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
                    "error" => self.serialize_error(r.0),
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

    /// 普通对象：**自有可枚举键逐键 `Get`**（Node 实测：访问器属性在克隆时
    /// 调用 getter 取返回值——`structuredClone({get a(){return 42}})` 得
    /// `{a:42}`；getter 抛错原样传播，返回函数则 DataCloneError）。
    ///
    /// 刻意**不复用** `own_entries`：后者把访问器函数值并入自有属性（服务于
    /// `Object.keys`/`JSON.stringify`，那些场景绝不能触发 getter），故克隆走
    /// [`Vm::own_clone_entries`] 单独取键，再对访问器键求值。
    fn serialize_object(&mut self, r: u32) -> Result<(), VmError> {
        // 先登记（循环引用经骨架回填）再写键值对
        self.objects.push(r);
        let pairs = self.vm.own_clone_entries(r as usize);
        self.tag(T_OBJECT);
        self.u32(pairs.len() as u32);
        for (k, slot) in pairs {
            let val = match slot {
                Some(v) => v,
                // 访问器键：`Get`（receiver = 源对象，Node 实测 getter 的
                // `this` 即源对象；每键求值一次）
                None => self
                    .vm
                    .get_property(Value::Object(aluka_core::ObjectRef(r)), &k)?,
            };
            self.str(&k);
            self.serialize_value(val)?;
        }
        Ok(())
    }

    /// Error 实例：`name` + `message` + flags + [stack] + [cause]。
    ///
    /// Node 实测口径：克隆体 `instanceof Error` 为真、`name`/`message` 保留
    /// （`message` 恒为字符串）、`cause` 深拷贝且仍不可枚举（cause 为函数则
    /// DataCloneError）、其余自有属性一律不携带（`e.extra` 丢弃）。
    fn serialize_error(&mut self, r: u32) -> Result<(), VmError> {
        // 先登记：cause 可循环指回自身
        self.objects.push(r);
        let idx = r as usize;
        let name = self
            .vm
            .own_text(idx, "name")
            .unwrap_or_else(|| "Error".to_owned());
        let message = self.vm.own_text(idx, "message").unwrap_or_default();
        let stack = self.vm.own_text(idx, "stack");
        let cause = self.vm.own_value(idx, "cause");
        let mut flags = 0u8;
        if stack.is_some() {
            flags |= 1;
        }
        if cause.is_some() {
            flags |= 2;
        }
        self.tag(T_ERROR);
        self.str(&name);
        self.str(&message);
        self.out.push(flags);
        if let Some(s) = stack {
            self.str(&s);
        }
        if let Some(c) = cause {
            self.serialize_value(c)?;
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

    /// ArrayBuffer / SharedArrayBuffer：克隆都携带全量字节（SAB 走 `T_SAB`
    /// 保留共享形态——Node 实测克隆体仍 `instanceof SharedArrayBuffer`）。
    /// transfer 集命中的缓冲由 [`serialize`] 在成功后统一 detach，此处不动源。
    fn serialize_ab(&mut self, r: u32) -> Result<(), VmError> {
        self.objects.push(r);
        let (data, detached, shared) = match self.vm.heap.get(r as usize) {
            Some(HeapObject::ArrayBuffer {
                data,
                detached,
                shared,
                ..
            }) => (data.clone(), *detached, *shared),
            _ => return Ok(()),
        };
        if detached {
            return Err(data_clone_error(
                self.vm,
                "ArrayBuffer could not be cloned.",
            ));
        }
        // SAB 永不入 transfer 集（校验期已拒绝），故不存在 detach 分支。
        self.tag(if shared { T_SAB } else { T_AB });
        self.u32(data.len() as u32);
        self.out.extend_from_slice(&data);
        Ok(())
    }

    /// TypedArray：克隆 = 复制视图字节区间为新缓冲上的视图；其底层 buffer 在
    /// transfer 集 → 整缓冲移交（视图字节区间与移交内容一致，收端按 kind/len
    /// 还原）。底层缓冲为共享缓冲时按 `T_TA` 的 flags bit0 标记，收端挂 SAB。
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
        let (data, shared) = match self.vm.heap.get(buffer.0 as usize) {
            Some(HeapObject::ArrayBuffer {
                data,
                detached,
                shared,
                ..
            }) => {
                if *detached {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
                if transfer {
                    // 整缓冲移交：offset 之外的字节也携带（视图区间内含）
                    (data.clone(), *shared)
                } else if data.len() >= view_end {
                    (data[byte_offset..view_end].to_vec(), *shared)
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
        // flags bit0 = 底层缓冲为共享缓冲（SAB）；Node 实测克隆出的视图仍挂 SAB
        self.out.push(u8::from(shared));
        self.u32(wire_offset as u32);
        self.u32(length as u32);
        self.u32(data.len() as u32);
        self.out.extend_from_slice(&data);
        Ok(())
    }

    /// DataView：载荷 = flags + offset + 字节数 + 视图长 + 字节（字节数与视图长
    /// 分离——transfer 时整缓冲字节 > 视图长）。
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
        let (data, shared) = match self.vm.heap.get(buffer.0 as usize) {
            Some(HeapObject::ArrayBuffer {
                data,
                detached,
                shared,
                ..
            }) => {
                if *detached {
                    return Err(data_clone_error(
                        self.vm,
                        "ArrayBuffer could not be cloned.",
                    ));
                }
                if transfer {
                    (data.clone(), *shared)
                } else if data.len() >= byte_offset + byte_length {
                    (
                        data[byte_offset..byte_offset + byte_length].to_vec(),
                        *shared,
                    )
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
        // 原 offset 保留。
        let wire_offset = if transfer { byte_offset } else { 0 };
        self.tag(T_DV);
        self.out.push(u8::from(shared));
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
            // Invalid Date：`getTime()` 为 NaN（Node 实测克隆 Invalid Date 后
            // `Number.isNaN(d.getTime())` 为 true）
            T_DATE_INVALID => {
                let d = self
                    .vm
                    .construct_date(&[Value::Number(f64::NAN)])
                    .map_err(|_| "clone: invalid date".to_owned())?;
                self.objects.push(d);
                Ok(d)
            }
            T_ERROR => self.error_value(),
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
            tag @ (T_AB | T_SAB) => {
                // T_SAB：克隆体仍为 SharedArrayBuffer（`shared` 标志保留；
                // 内存不共享属已登记偏离——跨 Worker 共享为 M5 未接线能力）
                let shared = tag == T_SAB;
                let len = self.u32()? as usize;
                let data = self.take(len)?;
                let ab = self.vm.alloc_array_buffer(data, shared, false, 0);
                self.objects.push(Value::Object(ab));
                Ok(Value::Object(ab))
            }
            T_TA => {
                let kind_raw = self.u8()?;
                let flags = self.u8()?;
                let byte_offset = self.u32()? as usize;
                let elems = self.u32()? as usize;
                let byte_len = self.u32()? as usize;
                let bytes = self.take(byte_len)?;
                let kind =
                    from_kind_tag(kind_raw).ok_or_else(|| "clone: bad ta kind".to_owned())?;
                // 克隆/转移的视图字节均从自身区间起（ta 原 offset 还原，
                // dv 原样）——新建缓冲承载区间字节；flags bit0 = 共享缓冲。
                let ab = self.vm.alloc_array_buffer(bytes, flags & 1 != 0, false, 0);
                let ta = self.vm.alloc_typed_array(kind, ab, byte_offset, elems);
                self.objects.push(Value::Object(ta));
                Ok(Value::Object(ta))
            }
            T_DV => {
                let flags = self.u8()?;
                let byte_offset = self.u32()? as usize;
                let data_len = self.u32()? as usize;
                let view_len = self.u32()? as usize;
                let bytes = self.take(data_len)?;
                let ab = self.vm.alloc_array_buffer(bytes, flags & 1 != 0, false, 0);
                let dv = self.vm.alloc_data_view(ab, byte_offset, view_len);
                self.objects.push(Value::Object(dv));
                Ok(Value::Object(dv))
            }
            other => Err(format!("clone: unknown tag {other}")),
        }
    }

    /// `T_ERROR` 重建：`error_prototype` 实例（`instanceof Error` 为真）+
    /// `message`/`name`/`stack`/`cause` 四者皆为**不可枚举**自有属性（Node
    /// 实测克隆体 `Object.keys` 为空集、`JSON.stringify` 为 `{}`）。
    fn error_value(&mut self) -> Result<Value, String> {
        let name = self.str()?;
        let message = self.str()?;
        let flags = self.u8()?;
        let err = self.vm.alloc_error_instance(&message);
        // 先登记：cause 可循环指回自身
        self.objects.push(Value::Object(err));
        self.vm.mark_non_enumerable(Value::Object(err), "message");
        let name_ref = self.vm.alloc_string(name);
        let _ = self
            .vm
            .set_property(Value::Object(err), "name", Value::Object(name_ref));
        self.vm.mark_non_enumerable(Value::Object(err), "name");
        if flags & 1 != 0 {
            let stack = self.str()?;
            let s = self.vm.alloc_string(stack);
            let _ = self
                .vm
                .set_property(Value::Object(err), "stack", Value::Object(s));
            self.vm.mark_non_enumerable(Value::Object(err), "stack");
        }
        if flags & 2 != 0 {
            let cause = self.value()?;
            let _ = self.vm.set_property(Value::Object(err), "cause", cause);
            self.vm.mark_non_enumerable(Value::Object(err), "cause");
        }
        Ok(Value::Object(err))
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

    /// 自有属性的文本形态（克隆 Error 的 name/message/stack 用）：堆字符串
    /// 直取原文，其余按 `format_value` 归一（Node 实测克隆体 `message` 恒为
    /// 字符串）。属性不存在（或已被删除）返回 `None`。
    fn own_text(&self, idx: usize, key: &str) -> Option<String> {
        let v = self.own_value(idx, key)?;
        if let Some(r) = v.as_object() {
            if let Some(HeapObject::String(s)) = self.heap.get(r.0 as usize) {
                return Some(s.clone());
            }
        }
        Some(self.format_value(v))
    }
}

/// `structuredClone(value[, { transfer }])`：全局结构化克隆。
///
/// 复用 worker `postMessage` / 同线程往返（`json_roundtrip`）同一套自描述
/// 序列化（`serialize` + `deserialize`），因此类型面、循环与共享引用、transfer
/// 移交与源 detach、以及不可克隆值的 `DataCloneError` 语义，与 worker 传值完全一致。
impl Vm {
    pub(crate) fn structured_clone(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let Some(value) = args.first().copied() else {
            // Node：TypeError: The value argument must be specified
            return Err(clone_type_error(
                self,
                "The value argument must be specified",
            ));
        };
        // `{ transfer: [...] }`：ArrayBuffer 或其视图的移交列表（可选）
        let transfer = match args.get(1).copied().map(|v| v.case()) {
            Some(ValueCase::Object(opts)) => {
                let arr = self
                    .get_property(Value::Object(opts), "transfer")
                    .unwrap_or(Value::Undefined);
                self.to_array_values(arr)
            }
            _ => Vec::new(),
        };
        let bytes = serialize(self, value, &transfer)?;
        deserialize(self, &bytes).map_err(|_| data_clone_error(self, "Object could not be cloned."))
    }
}

/// TypeError 错误对象（Node validator 文本形态：`name` = "TypeError"）。
fn clone_type_error(vm: &mut Vm, msg: &str) -> VmError {
    let obj = vm.alloc_ordinary();
    let n = vm.alloc_string("TypeError".to_owned());
    let _ = vm.set_property(Value::Object(obj), "name", Value::Object(n));
    let m = vm.alloc_string(msg.to_owned());
    let _ = vm.set_property(Value::Object(obj), "message", Value::Object(m));
    VmError::Thrown(Value::Object(obj))
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
