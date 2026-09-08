//! 类型化数组体系（ES2020+，语义对齐 Node.js 22 LTS）。
//!
//! 覆盖 `ArrayBuffer` / `SharedArrayBuffer` / `DataView` 与 11 种 TypedArray
//! 构造函数（`Int8`/`Uint8`/`Uint8Clamped`/`Int16`/`Uint16`/`Int32`/`Uint32`/
//! `Float32`/`Float64`/`BigInt64`/`BigUint64`）。字节缓冲为堆内
//! [`HeapObject::ArrayBuffer`]，TypedArray/DataView 持有视图句柄。
//!
//! 已知降级：`SharedArrayBuffer` 当前进程内共享（跨 Worker 共享待 M5 接线）；
//! ArrayBuffer 生命周期由分代 GC 管理（本模块只做视图访问）。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// 类型化数组元素类型（字节序一律按平台小端存储于 ArrayBuffer，
/// DataView 显式字节序访问例外——按规范逐字节编码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    /// `Int8Array`
    Int8,
    /// `Uint8Array`
    Uint8,
    /// `Uint8ClampedArray`（赋值钳制 [0,255] 并四舍五入取偶）
    Uint8Clamped,
    /// `Int16Array`
    Int16,
    /// `Uint16Array`
    Uint16,
    /// `Int32Array`
    Int32,
    /// `Uint32Array`
    Uint32,
    /// `Float32Array`
    Float32,
    /// `Float64Array`
    Float64,
    /// `BigInt64Array`
    BigInt64,
    /// `BigUint64Array`
    BigUint64,
}

impl TypedKind {
    /// 元素字节宽度。
    #[must_use]
    pub fn elem_size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    /// 构造函数名（`resolve_global` / `new` 分派键）。
    #[must_use]
    pub fn ctor_name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }

    /// 全部 11 种类型（构造器单例初始化遍历用）。
    #[must_use]
    pub fn all() -> [Self; 11] {
        [
            Self::Int8,
            Self::Uint8,
            Self::Uint8Clamped,
            Self::Int16,
            Self::Uint16,
            Self::Int32,
            Self::Uint32,
            Self::Float32,
            Self::Float64,
            Self::BigInt64,
            Self::BigUint64,
        ]
    }

    /// 构造函数名 → 元素类型（`do_construct` / 静态方法分派用）。
    #[must_use]
    pub fn by_ctor_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|k| k.ctor_name() == name)
    }

    /// 小端读取一个元素字节并按类型解码为原始位型。
    pub(crate) fn read_le(self, bytes: &[u8], off: usize) -> Element {
        match self {
            Self::Int8 => Element::I(bytes[off] as i8 as f64),
            Self::Uint8 | Self::Uint8Clamped => Element::I(bytes[off] as f64),
            Self::Int16 => {
                let v = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
                Element::I(v as f64)
            }
            Self::Uint16 => {
                let v = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
                Element::I(v as f64)
            }
            Self::Int32 => {
                let v = i32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                Element::I(v as f64)
            }
            Self::Uint32 => {
                let v = u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                Element::I(v as f64)
            }
            Self::Float32 => {
                let v = f32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                Element::I(v as f64)
            }
            Self::Float64 => {
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&bytes[off..off + 8]);
                Element::I(f64::from_le_bytes(b8))
            }
            Self::BigInt64 => {
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&bytes[off..off + 8]);
                Element::Big(i64::from_le_bytes(b8))
            }
            Self::BigUint64 => {
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&bytes[off..off + 8]);
                // 以位型承载 u64（显示端按无符号解释）
                Element::Big(u64::from_le_bytes(b8) as i64)
            }
        }
    }

    /// 小端写入一个元素（`v` 已按类型收窄/钳制）。
    fn write_le(self, bytes: &mut [u8], off: usize, v: Element) {
        match self {
            Self::Int8 => bytes[off] = v.as_i64() as i8 as u8,
            Self::Uint8 => bytes[off] = v.as_i64() as u8,
            Self::Uint8Clamped => bytes[off] = clamp_u8(v.as_f64()),
            Self::Int16 => {
                let b = (v.as_i64() as i16).to_le_bytes();
                bytes[off..off + 2].copy_from_slice(&b);
            }
            Self::Uint16 => {
                let b = (v.as_i64() as u16).to_le_bytes();
                bytes[off..off + 2].copy_from_slice(&b);
            }
            Self::Int32 => {
                let b = (v.as_i64() as i32).to_le_bytes();
                bytes[off..off + 4].copy_from_slice(&b);
            }
            Self::Uint32 => {
                let b = (v.as_i64() as u32).to_le_bytes();
                bytes[off..off + 4].copy_from_slice(&b);
            }
            Self::Float32 => {
                let b = (v.as_f64() as f32).to_le_bytes();
                bytes[off..off + 4].copy_from_slice(&b);
            }
            Self::Float64 => {
                let b = v.as_f64().to_le_bytes();
                bytes[off..off + 8].copy_from_slice(&b);
            }
            Self::BigInt64 | Self::BigUint64 => {
                let b = v.as_i64().to_le_bytes();
                bytes[off..off + 8].copy_from_slice(&b);
            }
        }
    }
}

/// 元素原始值（整数族以 f64 承载 JS Number，BigInt 族以 i64 承载）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Element {
    /// 数值族元素
    I(f64),
    /// BigInt 族元素
    Big(i64),
}

impl Vm {
    /// 元素原始值 → JS 值（BigUint64 按无符号位型渲染）。
    pub(crate) fn decode_element(
        &mut self,
        kind: crate::typed_array::TypedKind,
        elem: crate::typed_array::Element,
    ) -> Value {
        match (kind, elem) {
            (TypedKind::BigUint64, Element::Big(v)) => {
                Value::Object(self.alloc_bigint((v as u64).to_string()))
            }
            (_, Element::Big(v)) => Value::Object(self.alloc_bigint(v.to_string())),
            (_, Element::I(v)) => Value::Number(v),
        }
    }
}

impl Element {
    fn as_f64(self) -> f64 {
        match self {
            Self::I(v) => v,
            Self::Big(v) => v as f64,
        }
    }

    fn as_i64(self) -> i64 {
        match self {
            Self::I(v) => v as i64,
            Self::Big(v) => v,
        }
    }
}

/// `Uint8ClampedArray` 赋值钳制：`[0,255]` 外取边界、内四舍五入取偶
/// （规范 ToUint8Clamp；NaN → 0）。
fn clamp_u8(v: f64) -> u8 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        let f = v.floor();
        let frac = v - f;
        // 恰为 0.5 时取偶（银行家舍入）；其余四舍五入
        let r = if frac > 0.5 || (frac == 0.5 && (f as i64) % 2 != 0) {
            f + 1.0
        } else {
            f
        };
        r as u8
    }
}

impl Vm {
    /// 判断值是否为类型化数组视图。
    #[must_use]
    pub fn is_typed_array(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::TypedArray { .. }))
        )
    }

    /// 判断值是否为 ArrayBuffer / SharedArrayBuffer。
    #[must_use]
    pub fn is_array_buffer(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::ArrayBuffer { .. }))
        )
    }

    /// 判断值是否为 DataView。
    #[must_use]
    pub fn is_data_view(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::DataView { .. }))
        )
    }

    /// 读取视图三要素 `(kind, buffer, byte_offset, length)`；非 TypedArray 返回 `None`。
    pub(crate) fn ta_parts(
        &self,
        r: ObjectRef,
    ) -> Option<(crate::typed_array::TypedKind, ObjectRef, usize, usize)> {
        match self.heap.get(r.0 as usize) {
            Some(HeapObject::TypedArray {
                kind,
                buffer,
                byte_offset,
                length,
            }) => Some((*kind, *buffer, *byte_offset, *length)),
            _ => None,
        }
    }

    /// 分离校验：buffer 已分离时抛 TypeError（TypedArray/DataView 共用）。
    pub(crate) fn check_detached(&mut self, buffer: ObjectRef) -> Result<(), VmError> {
        let detached = matches!(
            self.heap.get(buffer.0 as usize),
            Some(HeapObject::ArrayBuffer { detached: true, .. })
        );
        if detached {
            let msg = "Cannot perform operation on a detached ArrayBuffer";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "TypeError"),
            )));
        }
        Ok(())
    }

    /// 读取 TypedArray 下标元素（越界 → undefined）。
    pub(crate) fn ta_get(&mut self, r: ObjectRef, i: usize) -> Result<Value, VmError> {
        let Some((kind, buffer, byte_offset, length)) = self.ta_parts(r) else {
            return Ok(Value::Undefined);
        };
        self.check_detached(buffer)?;
        if i >= length {
            return Ok(Value::Undefined);
        }
        let off = byte_offset + i * kind.elem_size();
        let Some(HeapObject::ArrayBuffer { data, .. }) = self.heap.get(buffer.0 as usize) else {
            return Ok(Value::Undefined);
        };
        let elem = kind.read_le(data, off);
        Ok(self.decode_element(kind, elem))
    }

    /// 写入 TypedArray 下标元素（越界静默忽略；值按类型收窄/钳制）。
    pub(crate) fn ta_set(&mut self, r: ObjectRef, i: usize, val: Value) -> Result<(), VmError> {
        let Some((kind, buffer, byte_offset, length)) = self.ta_parts(r) else {
            return Ok(());
        };
        self.check_detached(buffer)?;
        if i >= length {
            return Ok(());
        }
        let elem = self.convert_element(kind, val)?;
        let off = byte_offset + i * kind.elem_size();
        let Some(HeapObject::ArrayBuffer { data, .. }) = self.heap.get_mut(buffer.0 as usize)
        else {
            return Ok(());
        };
        if off + kind.elem_size() <= data.len() {
            kind.write_le(data, off, elem);
        }
        Ok(())
    }

    /// JS 值 → 元素原始值（BigInt 族收窄 int64；数值族经 ToNumber）。
    fn convert_element(
        &mut self,
        kind: crate::typed_array::TypedKind,
        val: Value,
    ) -> Result<Element, VmError> {
        if matches!(
            kind,
            crate::typed_array::TypedKind::BigInt64 | crate::typed_array::TypedKind::BigUint64
        ) {
            // BigInt 族：接受 BigInt 与整数值；BigUint64 按无符号回绕
            if let Value::Object(r) = val {
                if let Some(HeapObject::BigInt(text)) = self.heap.get(r.0 as usize) {
                    let parsed = match kind {
                        crate::typed_array::TypedKind::BigUint64 => text
                            .parse::<u128>()
                            .map(|v| (v as u64) as i64)
                            .or_else(|_| text.parse::<i64>())
                            .unwrap_or(0),
                        _ => text.parse::<i64>().unwrap_or(0),
                    };
                    return Ok(Element::Big(parsed));
                }
            }
            let n = crate::ops::to_number(val);
            let i = if n.is_nan() || n.is_infinite() {
                0i64
            } else {
                n.trunc() as i64
            };
            return Ok(Element::Big(i));
        }
        Ok(Element::I(crate::ops::to_number(val)))
    }

    /// 元素个数（length）。
    pub(crate) fn ta_length(&self, r: ObjectRef) -> usize {
        self.ta_parts(r).map(|(_, _, _, l)| l).unwrap_or(0)
    }

    /// 构造 `new ArrayBuffer(length[, opts])` / `new SharedArrayBuffer(length)`。
    pub(crate) fn construct_array_buffer(
        &mut self,
        args: &[Value],
        shared: bool,
    ) -> Result<Value, VmError> {
        let len = args
            .first()
            .map(|v| crate::ops::to_number(*v))
            .map(|n| {
                if n.is_nan() || n < 0.0 {
                    0usize
                } else {
                    n as usize
                }
            })
            .unwrap_or(0);
        // resizable 支持：opts.resizable === true 时记录 maxByteLength
        let mut resizable = false;
        let mut max = len;
        if let Some(Value::Object(opts)) = args.get(1).copied() {
            if let Ok(v) = self.get_property(Value::Object(opts), "resizable") {
                if self.truthy(v) {
                    resizable = true;
                }
            }
            if let Ok(v) = self.get_property(Value::Object(opts), "maxByteLength") {
                let m = crate::ops::to_number(v);
                if !m.is_nan() && m as usize > max {
                    max = m as usize;
                }
            }
        }
        Ok(Value::Object(self.alloc_array_buffer(
            vec![0u8; len],
            shared,
            resizable,
            max,
        )))
    }

    /// `new <TypedArray>(arg)`：length | 类型化数组 | 数组/可迭代 | buffer[, off[, len]]。
    pub(crate) fn construct_typed_array(
        &mut self,
        kind: crate::typed_array::TypedKind,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let first = args.first().copied().unwrap_or(Value::Undefined);
        match first {
            // 数字：分配 length 个零元素
            Value::Number(_) | Value::Undefined => {
                let len = match first {
                    Value::Number(n) if n > 0.0 => n as usize,
                    _ => 0,
                };
                let buf =
                    self.alloc_array_buffer(vec![0u8; len * kind.elem_size()], false, false, 0);
                Ok(Value::Object(self.alloc_typed_array(kind, buf, 0, len)))
            }
            Value::Object(r) => match self.heap.get(r.0 as usize) {
                // buffer[, byteOffset[, length]]：在既有缓冲上建视图
                Some(HeapObject::ArrayBuffer { shared: _, .. }) => {
                    let buf_len = match self.heap.get(r.0 as usize) {
                        Some(HeapObject::ArrayBuffer { data, .. }) => data.len(),
                        _ => 0,
                    };
                    let off = args
                        .get(1)
                        .map(|v| crate::ops::to_number(*v))
                        .map(|n| {
                            if n.is_nan() || n < 0.0 {
                                0usize
                            } else {
                                n as usize
                            }
                        })
                        .unwrap_or(0);
                    if off > buf_len || off % kind.elem_size() != 0 {
                        let msg = "start offset of Array Buffer is incorrect";
                        return Err(VmError::Thrown(Value::Object(
                            self.alloc_typed_error(msg, "RangeError"),
                        )));
                    }
                    let len = match args.get(2) {
                        Some(v) if !matches!(v, Value::Undefined) => {
                            let n = crate::ops::to_number(*v);
                            if n.is_nan() || n < 0.0 { 0 } else { n as usize }
                        }
                        _ => (buf_len - off) / kind.elem_size(),
                    };
                    if off + len * kind.elem_size() > buf_len {
                        let msg = "invalid typed array length";
                        return Err(VmError::Thrown(Value::Object(
                            self.alloc_typed_error(msg, "RangeError"),
                        )));
                    }
                    Ok(Value::Object(self.alloc_typed_array(kind, r, off, len)))
                }
                // 类型化数组：复制元素
                Some(HeapObject::TypedArray { .. }) => {
                    let src_len = self.ta_length(r);
                    let mut elems = Vec::with_capacity(src_len);
                    for i in 0..src_len {
                        elems.push(self.ta_get(r, i)?);
                    }
                    self.typed_array_from_values(kind, &elems)
                }
                // 数组/普通对象：逐下标取元素
                _ => {
                    let vals = self.to_array_values(first);
                    let mut elems = Vec::with_capacity(vals.len());
                    for v in &vals {
                        elems.push(*v);
                    }
                    self.typed_array_from_values(kind, &elems)
                }
            },
            _ => {
                let buf = self.alloc_array_buffer(vec![0u8; 0], false, false, 0);
                Ok(Value::Object(self.alloc_typed_array(kind, buf, 0, 0)))
            }
        }
    }

    /// 以 JS 值列表构造类型化数组（`from`/`of`/数组复制共用）。
    fn typed_array_from_values(
        &mut self,
        kind: crate::typed_array::TypedKind,
        vals: &[Value],
    ) -> Result<Value, VmError> {
        let len = vals.len();
        let buf = self.alloc_array_buffer(vec![0u8; len * kind.elem_size()], false, false, 0);
        let ta = self.alloc_typed_array(kind, buf, 0, len);
        for (i, v) in vals.iter().enumerate() {
            self.ta_set(ta, i, *v)?;
        }
        Ok(Value::Object(ta))
    }

    /// `new DataView(buffer[, byteOffset[, byteLength]])`。
    pub(crate) fn construct_data_view(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let Some(Value::Object(buffer)) = args.first().copied() else {
            let msg = "first argument to DataView constructor must be an ArrayBuffer";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "TypeError"),
            )));
        };
        if !matches!(
            self.heap.get(buffer.0 as usize),
            Some(HeapObject::ArrayBuffer { .. })
        ) {
            let msg = "first argument to DataView constructor must be an ArrayBuffer";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "TypeError"),
            )));
        }
        let buf_len = match self.heap.get(buffer.0 as usize) {
            Some(HeapObject::ArrayBuffer { data, .. }) => data.len(),
            _ => 0,
        };
        let off = args
            .get(1)
            .map(|v| crate::ops::to_number(*v))
            .map(|n| {
                if n.is_nan() || n < 0.0 {
                    0usize
                } else {
                    n as usize
                }
            })
            .unwrap_or(0);
        if off > buf_len {
            let msg = "start offset of ArrayBuffer is less than 0 or larger than its size";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "RangeError"),
            )));
        }
        let blen = match args.get(2) {
            Some(v) if !matches!(v, Value::Undefined) => {
                let n = crate::ops::to_number(*v);
                if n.is_nan() || n < 0.0 { 0 } else { n as usize }
            }
            _ => buf_len - off,
        };
        if off + blen > buf_len {
            let msg = "invalid DataView length";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "RangeError"),
            )));
        }
        Ok(Value::Object(self.alloc_data_view(buffer, off, blen)))
    }

    /// TypedArray / DataView / ArrayBuffer 的方法统一分派。
    ///
    /// 返回 `None` 表示 receiver 不是本体系对象或方法未实现（调用方走
    /// 既有路径）。
    pub(crate) fn typed_array_dispatch(
        &mut self,
        receiver: Value,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, VmError>> {
        let Value::Object(r) = receiver else {
            return None;
        };
        match self.heap.get(r.0 as usize) {
            Some(HeapObject::TypedArray { .. }) => Some(self.typed_array_method(r, method, args)),
            Some(HeapObject::DataView { .. }) => Some(self.data_view_method(r, method, args)),
            Some(HeapObject::ArrayBuffer { .. }) => Some(self.array_buffer_method(r, method, args)),
            _ => None,
        }
    }

    /// TypedArray 原型方法实现（`ta` 为接收者句柄）。
    fn typed_array_method(
        &mut self,
        ta: ObjectRef,
        method: &str,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let this = Value::Object(ta);
        let arg_num = |i: usize| -> Option<f64> { args.get(i).map(|v| crate::ops::to_number(*v)) };
        match method {
            // 下标元素访问
            "at" => {
                let len = self.ta_length(ta) as f64;
                let n = arg_num(0).unwrap_or(f64::NAN);
                let i = if n < 0.0 { len + n } else { n };
                if i.is_nan() || i < 0.0 || i >= len {
                    return Ok(Value::Undefined);
                }
                self.ta_get(ta, i as usize)
            }
            "length" => Ok(Value::Number(self.ta_length(ta) as f64)),
            // 迭代协议（for...of 与 keys/values/entries 共用）
            "keys" | "values" | "entries" => {
                let kind = match method {
                    "keys" => "keys",
                    "entries" => "entries",
                    _ => "values",
                };
                // 借道数组迭代器：物化元素快照数组后迭代
                let elems = self.ta_to_values(ta)?;
                let snapshot = self.alloc_array(elems);
                Ok(self.alloc_array_iterator_kind(snapshot, kind))
            }
            "join" => {
                let sep = match args.first() {
                    Some(Value::Undefined) | None => ",".to_owned(),
                    Some(v) => self.format_value(*v),
                };
                let elems = self.ta_to_values(ta)?;
                let items: Vec<String> = elems
                    .iter()
                    .map(|v| match v {
                        Value::Undefined | Value::Null => String::new(),
                        x => self.format_value(*x),
                    })
                    .collect();
                Ok(Value::Object(self.alloc_string(items.join(&sep))))
            }
            "toString" | "toLocaleString" => {
                let elems = self.ta_to_values(ta)?;
                let items: Vec<String> = elems.iter().map(|v| self.format_value(*v)).collect();
                Ok(Value::Object(self.alloc_string(items.join(","))))
            }
            "indexOf" => {
                let needle = args.first().copied().unwrap_or(Value::Undefined);
                let elems = self.ta_to_values(ta)?;
                let pos = elems
                    .iter()
                    .position(|e| self.values_content_eq(*e, needle))
                    .map(|p| p as f64)
                    .unwrap_or(-1.0);
                Ok(Value::Number(pos))
            }
            "lastIndexOf" => {
                let needle = args.first().copied().unwrap_or(Value::Undefined);
                let elems = self.ta_to_values(ta)?;
                let pos = elems
                    .iter()
                    .rposition(|e| self.values_content_eq(*e, needle))
                    .map(|p| p as f64)
                    .unwrap_or(-1.0);
                Ok(Value::Number(pos))
            }
            "includes" => {
                let needle = args.first().copied().unwrap_or(Value::Undefined);
                let elems = self.ta_to_values(ta)?;
                let found = elems.iter().any(|e| {
                    if let (Value::Number(x), Value::Number(y)) = (e, needle) {
                        if x.is_nan() && y.is_nan() {
                            return true;
                        }
                    }
                    e == &needle
                });
                Ok(Value::Boolean(found))
            }
            "reverse" => {
                let mut elems = self.ta_to_values(ta)?;
                elems.reverse();
                for (i, v) in elems.iter().enumerate() {
                    self.ta_set(ta, i, *v)?;
                }
                Ok(this)
            }
            "fill" => {
                let fill = args.first().copied().unwrap_or(Value::Undefined);
                let len = self.ta_length(ta);
                let s = normalized_index(arg_num(1), len);
                let e = match args.get(2) {
                    Some(v) if !matches!(v, Value::Undefined) => {
                        normalized_index(Some(crate::ops::to_number(*v)), len)
                    }
                    _ => len,
                };
                for i in s..e {
                    self.ta_set(ta, i, fill)?;
                }
                Ok(this)
            }
            "copyWithin" => {
                let elems = self.ta_to_values(ta)?;
                let len = elems.len();
                let target = normalized_index(arg_num(0), len);
                let start = normalized_index(arg_num(1), len);
                let end = match args.get(2) {
                    Some(v) if !matches!(v, Value::Undefined) => {
                        normalized_index(Some(crate::ops::to_number(*v)), len)
                    }
                    _ => len,
                };
                let count = (end - start).min(len - target);
                for i in 0..count {
                    self.ta_set(ta, target + i, elems[start + i])?;
                }
                Ok(this)
            }
            "subarray" | "slice" => {
                let (kind, buffer, byte_offset, length) =
                    self.ta_parts(ta).ok_or(VmError::LocalOutOfRange)?;
                self.check_detached(buffer)?;
                let begin = normalized_index(arg_num(0), length);
                let end = match args.get(1) {
                    Some(v) if !matches!(v, Value::Undefined) => {
                        normalized_index(Some(crate::ops::to_number(*v)), length)
                    }
                    _ => length,
                };
                if method == "subarray" {
                    // 视图共享底层缓冲
                    let byte = byte_offset + begin * kind.elem_size();
                    let len = end.saturating_sub(begin);
                    Ok(Value::Object(
                        self.alloc_typed_array(kind, buffer, byte, len),
                    ))
                } else {
                    // slice 复制元素到新缓冲
                    let mut vals = Vec::with_capacity(end.saturating_sub(begin));
                    for i in begin..end {
                        vals.push(self.ta_get(ta, i)?);
                    }
                    self.typed_array_from_values(kind, &vals)
                }
            }
            "set" => {
                // ta.set(array|typedArray[, offset])
                let offset = arg_num(1).unwrap_or(0.0).max(0.0) as usize;
                let src = args.first().copied().unwrap_or(Value::Undefined);
                let vals = if let Value::Object(sr) = src {
                    if self.ta_parts(sr).is_some() {
                        self.ta_to_values(sr)?
                    } else {
                        self.to_array_values(src)
                    }
                } else {
                    self.to_array_values(src)
                };
                for (i, v) in vals.into_iter().enumerate() {
                    self.ta_set(ta, offset + i, v)?;
                }
                Ok(Value::Undefined)
            }
            // 回调族
            "forEach" | "map" | "filter" | "find" | "findIndex" | "findLast" | "findLastIndex"
            | "some" | "every" => {
                let cb = args.first().copied().unwrap_or(Value::Undefined);
                let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
                let elems = self.ta_to_values(ta)?;
                let this = Value::Object(ta);
                let mut mapped: Vec<Value> = Vec::with_capacity(elems.len());
                let mut filtered: Vec<Value> = Vec::with_capacity(elems.len());
                let mut hit: Option<(usize, Value)> = None;
                for (i, e) in elems.iter().enumerate() {
                    let r =
                        self.invoke_callable(cb, this_arg, &[*e, Value::Number(i as f64), this])?;
                    let tr = self.truthy(r);
                    match method {
                        "map" => mapped.push(r),
                        "filter" if tr => filtered.push(*e),
                        "find" | "findIndex" | "findLast" | "findLastIndex" if tr => match hit {
                            None => hit = Some((i, *e)),
                            Some(_) if method.starts_with("findLast") => {
                                hit = Some((i, *e));
                            }
                            _ => {}
                        },
                        "some" if tr => return Ok(Value::Boolean(true)),
                        "every" if !tr => return Ok(Value::Boolean(false)),
                        _ => {}
                    }
                }
                match method {
                    "map" => self.typed_array_from_values(
                        self.ta_parts(ta)
                            .map(|(k, ..)| k)
                            .unwrap_or(crate::typed_array::TypedKind::Uint8),
                        &mapped,
                    ),
                    "filter" => self.typed_array_from_values(
                        self.ta_parts(ta)
                            .map(|(k, ..)| k)
                            .unwrap_or(crate::typed_array::TypedKind::Uint8),
                        &filtered,
                    ),
                    "find" => Ok(hit.map(|(_, v)| v).unwrap_or(Value::Undefined)),
                    "findLast" => Ok(hit.map(|(_, v)| v).unwrap_or(Value::Undefined)),
                    "findIndex" | "findLastIndex" => {
                        Ok(Value::Number(hit.map(|(i, _)| i as f64).unwrap_or(-1.0)))
                    }
                    "some" => Ok(Value::Boolean(false)),
                    "every" => Ok(Value::Boolean(true)),
                    _ => Ok(Value::Undefined),
                }
            }
            "reduce" | "reduceRight" => {
                let cb = args.first().copied().unwrap_or(Value::Undefined);
                let mut elems = self.ta_to_values(ta)?;
                if method == "reduceRight" {
                    elems.reverse();
                }
                let this = Value::Object(ta);
                let mut acc = match args.get(1) {
                    Some(v) => *v,
                    None => {
                        let Some(first) = elems.first() else {
                            let msg = "Reduce of empty array with no initial value";
                            return Err(VmError::Thrown(Value::Object(
                                self.alloc_typed_error(msg, "TypeError"),
                            )));
                        };
                        *first
                    }
                };
                for (i, e) in elems.iter().enumerate() {
                    acc = self.invoke_callable(
                        cb,
                        Value::Undefined,
                        &[acc, *e, Value::Number(i as f64), this],
                    )?;
                }
                Ok(acc)
            }
            "sort" => {
                let cmp = args.first().copied().unwrap_or(Value::Undefined);
                let mut elems = self.ta_to_values(ta)?;
                if matches!(cmp, Value::Undefined) {
                    // 数值排序（TypedArray 默认数值序，区别于 Array 字典序）
                    elems.sort_by(|a, b| {
                        let x = crate::ops::to_number(*a);
                        let y = crate::ops::to_number(*b);
                        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                    });
                } else {
                    // 比较器排序（插入序稳定性足够）
                    for i in 1..elems.len() {
                        let mut j = i;
                        while j > 0 {
                            let lt = self.invoke_callable(
                                cmp,
                                Value::Undefined,
                                &[elems[j - 1], elems[j]],
                            )?;
                            if self.truthy(lt) {
                                elems.swap(j - 1, j);
                                j -= 1;
                            } else {
                                break;
                            }
                        }
                    }
                }
                for (i, v) in elems.iter().enumerate() {
                    self.ta_set(ta, i, *v)?;
                }
                Ok(this)
            }
            _ => Ok(Value::Undefined),
        }
    }

    /// 读取全部元素为 JS 值快照。
    pub(crate) fn ta_to_values(&mut self, ta: ObjectRef) -> Result<Vec<Value>, VmError> {
        let len = self.ta_length(ta);
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(self.ta_get(ta, i)?);
        }
        Ok(out)
    }

    /// DataView 方法（`get<Type>(byteOffset[, littleEndian])` /
    /// `set<Type>(byteOffset, value[, littleEndian])`；默认大端，按规范）。
    fn data_view_method(
        &mut self,
        dv: ObjectRef,
        method: &str,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let Some(HeapObject::DataView {
            buffer,
            byte_offset,
            byte_length,
        }) = self.heap.get(dv.0 as usize).cloned()
        else {
            return Ok(Value::Undefined);
        };
        self.check_detached(buffer)?;
        // 方法名 → (宽度, 读取器类别)
        let spec = data_view_spec(method);
        let Some((size, is_set, kind)) = spec else {
            return Ok(Value::Undefined);
        };
        let off = byte_offset
            + args
                .first()
                .map(|v| crate::ops::to_number(*v))
                .map(|n| {
                    if n.is_nan() || n < 0.0 {
                        0usize
                    } else {
                        n as usize
                    }
                })
                .unwrap_or(0);
        if off + size > byte_offset + byte_length {
            let msg = format!(
                "Offset is outside the bounds of the DataView (off={}, size={})",
                off - byte_offset,
                size
            );
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(&msg, "RangeError"),
            )));
        }
        let little = args
            .get(if is_set { 2 } else { 1 })
            .copied()
            .is_some_and(|v| self.truthy(v));
        let mut raw = [0u8; 8];
        if is_set {
            let value = args.get(1).copied().unwrap_or(Value::Undefined);
            // 写入：值按类型编码后以指定字节序落盘
            let enc = match kind {
                DvKind::I8 => vec![crate::ops::to_number(value) as i8 as u8],
                DvKind::U8 => vec![crate::ops::to_number(value) as u8],
                DvKind::I16 => (crate::ops::to_number(value) as i16).to_le_bytes().to_vec(),
                DvKind::U16 => (crate::ops::to_number(value) as u16).to_le_bytes().to_vec(),
                DvKind::I32 => (crate::ops::to_number(value) as i32).to_le_bytes().to_vec(),
                DvKind::U32 => (crate::ops::to_number(value) as u32).to_le_bytes().to_vec(),
                DvKind::F32 => ((crate::ops::to_number(value) as f32).to_le_bytes()).to_vec(),
                DvKind::F64 => crate::ops::to_number(value).to_le_bytes().to_vec(),
                DvKind::I64 => bigint_of(self, value)?.to_le_bytes().to_vec(),
                DvKind::U64 => (bigint_of(self, value)? as u64).to_le_bytes().to_vec(),
            };
            let mut bytes = enc;
            if !little {
                bytes.reverse();
            }
            if let Some(HeapObject::ArrayBuffer { data, .. }) = self.heap.get_mut(buffer.0 as usize)
            {
                data[off..off + bytes.len()].copy_from_slice(&bytes);
            }
            Ok(Value::Undefined)
        } else {
            if let Some(HeapObject::ArrayBuffer { data, .. }) = self.heap.get_mut(buffer.0 as usize)
            {
                raw[..size].copy_from_slice(&data[off..off + size]);
            }
            let mut le_raw = raw;
            if !little {
                le_raw[..size].reverse();
            }
            let val =
                match kind {
                    DvKind::I8 => Value::Number(le_raw[0] as i8 as f64),
                    DvKind::U8 => Value::Number(le_raw[0] as f64),
                    DvKind::I16 => Value::Number(i16::from_le_bytes([le_raw[0], le_raw[1]]) as f64),
                    DvKind::U16 => Value::Number(u16::from_le_bytes([le_raw[0], le_raw[1]]) as f64),
                    DvKind::I32 => Value::Number(i32::from_le_bytes([
                        le_raw[0], le_raw[1], le_raw[2], le_raw[3],
                    ]) as f64),
                    DvKind::U32 => Value::Number(u32::from_le_bytes([
                        le_raw[0], le_raw[1], le_raw[2], le_raw[3],
                    ]) as f64),
                    DvKind::F32 => {
                        let mut b4 = [0u8; 4];
                        b4.copy_from_slice(&le_raw[..4]);
                        Value::Number(f32::from_le_bytes(b4) as f64)
                    }
                    DvKind::F64 => {
                        let mut b8 = [0u8; 8];
                        b8.copy_from_slice(&le_raw[..8]);
                        Value::Number(f64::from_le_bytes(b8))
                    }
                    DvKind::I64 => {
                        let mut b8 = [0u8; 8];
                        b8.copy_from_slice(&le_raw[..8]);
                        Value::Object(self.alloc_bigint(i64::from_le_bytes(b8).to_string()))
                    }
                    DvKind::U64 => {
                        let mut b8 = [0u8; 8];
                        b8.copy_from_slice(&le_raw[..8]);
                        Value::Object(self.alloc_bigint(u64::from_le_bytes(b8).to_string()))
                    }
                };
            Ok(val)
        }
    }

    /// ArrayBuffer / SharedArrayBuffer 实例方法（`slice`）。
    fn array_buffer_method(
        &mut self,
        ab: ObjectRef,
        method: &str,
        args: &[Value],
    ) -> Result<Value, VmError> {
        if method != "slice" {
            return Ok(Value::Undefined);
        }
        let (data, shared) = match self.heap.get(ab.0 as usize) {
            Some(HeapObject::ArrayBuffer { data, shared, .. }) => (data.clone(), *shared),
            _ => (Vec::new(), false),
        };
        let len = data.len();
        let begin = normalized_index(args.first().map(|v| crate::ops::to_number(*v)), len);
        let end = match args.get(1) {
            Some(v) if !matches!(v, Value::Undefined) => {
                normalized_index(Some(crate::ops::to_number(*v)), len)
            }
            _ => len,
        };
        let sliced = data[begin..end.max(begin)].to_vec();
        Ok(Value::Object(
            self.alloc_array_buffer(sliced, shared, false, 0),
        ))
    }
}

/// DataView 方法名解析：`(宽度, 是否 setter, 类别)`。
fn data_view_spec(method: &str) -> Option<(usize, bool, DvKind)> {
    let (name, is_set) = match method.strip_prefix("set") {
        Some(rest) => (rest, true),
        None => (method.strip_prefix("get")?, false),
    };
    let kind = match name {
        "Int8" => DvKind::I8,
        "Uint8" => DvKind::U8,
        "Int16" => DvKind::I16,
        "Uint16" => DvKind::U16,
        "Int32" => DvKind::I32,
        "Uint32" => DvKind::U32,
        "Float32" => DvKind::F32,
        "Float64" => DvKind::F64,
        "BigInt64" => DvKind::I64,
        "BigUint64" => DvKind::U64,
        _ => return None,
    };
    Some((kind.size(), is_set, kind))
}

/// DataView 元素类别。
#[derive(Debug, Clone, Copy)]
enum DvKind {
    /// `Int8`
    I8,
    /// `Uint8`
    U8,
    /// `Int16`
    I16,
    /// `Uint16`
    U16,
    /// `Int32`
    I32,
    /// `Uint32`
    U32,
    /// `Float32`
    F32,
    /// `Float64`
    F64,
    /// `BigInt64`
    I64,
    /// `BigUint64`
    U64,
}

impl DvKind {
    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 | Self::I64 | Self::U64 => 8,
        }
    }
}

/// DataView BigInt 族取值：BigInt 对象或整数值 → i64。
fn bigint_of(vm: &mut Vm, val: Value) -> Result<i64, VmError> {
    if let Value::Object(r) = val {
        if let Some(HeapObject::BigInt(text)) = vm.heap.get(r.0 as usize) {
            return Ok(text.parse::<i64>().unwrap_or(0));
        }
    }
    let n = crate::ops::to_number(val);
    Ok(if n.is_nan() || n.is_infinite() {
        0
    } else {
        n.trunc() as i64
    })
}

/// 归一化下标：负值从尾部计数、NaN 视为 0、钳制到 `[0, len]`。
fn normalized_index(n: Option<f64>, len: usize) -> usize {
    let n = n.unwrap_or(f64::NAN);
    let raw = if n.is_nan() {
        0.0
    } else if n < 0.0 {
        len as f64 + n
    } else {
        n
    };
    if raw.is_nan() || raw <= 0.0 {
        0
    } else if raw >= len as f64 {
        len
    } else {
        raw as usize
    }
}

impl Vm {
    /// TypedArray 构造器静态方法分派（`from` / `of` / `isTypedArray`）。
    ///
    /// receiver 必须为 TA 构造器（NativeCtor 名命中 [`TypedKind::by_ctor_name`]）
    /// 或 ArrayBuffer 构造器（`isView`）。未命中返回 `None`。
    pub(crate) fn typed_array_statics(
        &mut self,
        receiver: Value,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, VmError>> {
        let Value::Object(r) = receiver else {
            return None;
        };
        let Some(HeapObject::NativeCtor { name, .. }) = self.heap.get(r.0 as usize) else {
            return None;
        };
        let name = name.clone();
        if let Some(kind) = TypedKind::by_ctor_name(&name) {
            match method {
                "from" => {
                    // from(source[, mapFn[, thisArg]])：数组/可迭代 → 新 TA
                    let src = args.first().copied().unwrap_or(Value::Undefined);
                    let mut vals = self.to_array_values(src);
                    if let Some(map_fn) = args.get(1).copied() {
                        if !matches!(map_fn, Value::Undefined) {
                            let this_arg = args.get(2).copied().unwrap_or(Value::Undefined);
                            let mut mapped = Vec::with_capacity(vals.len());
                            for (i, v) in vals.iter().enumerate() {
                                let r = match self.invoke_callable(
                                    map_fn,
                                    this_arg,
                                    &[*v, Value::Number(i as f64)],
                                ) {
                                    Ok(r) => r,
                                    Err(e) => return Some(Err(e)),
                                };
                                mapped.push(r);
                            }
                            vals = mapped;
                        }
                    }
                    return Some(self.typed_array_from_values(kind, &vals));
                }
                "of" => {
                    return Some(self.typed_array_from_values(kind, args));
                }
                "isTypedArray" => {
                    let v = args.first().copied().unwrap_or(Value::Undefined);
                    return Some(Ok(Value::Boolean(self.is_typed_array(v))));
                }
                _ => return None,
            }
        }
        if name == "ArrayBuffer" && method == "isView" {
            let v = args.first().copied().unwrap_or(Value::Undefined);
            let is_view = self.is_typed_array(v) || self.is_data_view(v);
            return Some(Ok(Value::Boolean(is_view)));
        }
        None
    }
}
