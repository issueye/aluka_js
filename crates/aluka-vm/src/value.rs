//! 虚拟机运行时值表示与上值句柄定义。
//!
//! # M6.2：8 字节 NaN-box 表示
//!
//! `Value` 从 16 字节 Tagged Enum 切换为 **8 字节 NaN-box 机器字**
//! （`#[repr(transparent)] u64`），编码规则与 `aluka-jit` 的 `valbox`
//! （JSC 风格）完全一致——机器码与解释器共用同一值域：
//! - **Number**：f64 比特直存，NaN 入盒时规范化到 `0x7FF8_0000_0000_0000`；
//! - **非数值**：`0xFFF7_0000_0000_0000 | tag`，tag 占最低字节，
//!   ObjectRef 占 8..=39 位（undefined=0 / null=1 / false=2 / true=3 / object=4）。
//!
//! # 兼容层（关键：构造点零改动）
//!
//! - **同名关联函数**充当构造器：`Value::Number(x)` / `Value::Boolean(b)` /
//!   `Value::Object(r)` 的调用形态与旧元组变体构造**逐字相同**
//!   （`#[allow(non_snake_case)]`）；
//! - **关联常量** `Value::Undefined` / `Value::Null`：既是构造表达式也是
//!   **合法模式**（const pattern），旧 `match v { Value::Null => … }` 零改动；
//! - 载荷**解构模式**（`Value::Number(n) => …`）改为访问器：
//!   `v.as_number()` / `v.as_object()` / `v.as_bool()` / `v.kind()`。

use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::rc::Rc;

/// tag 空间前缀：top16 == 0xFFF7（与 `aluka_jit::valbox` 同源）。
const TAG_PREFIX: u64 = 0xFFF7_0000_0000_0000;
/// 数字判定掩码：`(bits & TAG_MASK) != TAG_PREFIX` 即数字。
const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
/// 规范化 NaN。
const NAN_CANONICAL: u64 = 0x7FF8_0000_0000_0000;

const TAG_UNDEFINED: u64 = 0;
const TAG_NULL: u64 = 1;
const TAG_FALSE: u64 = 2;
const TAG_TRUE: u64 = 3;
const TAG_OBJECT: u64 = 4;

/// 运行时求值结果：8 字节 NaN-box 机器字。
///
/// 语义面与旧 Tagged Enum 完全一致（5 类：undefined / null / bool /
/// f64 / 堆对象句柄）；`PartialEq` 为**位级相等**（NaN 已在入盒时规范化，
/// `-0.0` 与 `0.0` 位级不同——JS 语义的数值相等由 `ops` 的专用运算实现，
/// 不依赖本 trait）。
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(transparent)]
pub struct Value(u64);

impl Value {
    /// `undefined`（关联常量：构造与模式双兼容）。
    #[allow(non_upper_case_globals)]
    pub const Undefined: Value = Value(TAG_PREFIX | TAG_UNDEFINED);
    /// `null`（关联常量：构造与模式双兼容）。
    #[allow(non_upper_case_globals)]
    pub const Null: Value = Value(TAG_PREFIX | TAG_NULL);

    /// 构造布尔值（false=tag2 / true=tag3，两值编码不同）。
    #[allow(non_snake_case)]
    #[must_use]
    #[inline]
    pub fn Boolean(b: bool) -> Self {
        Self(TAG_PREFIX | if b { TAG_TRUE } else { TAG_FALSE })
    }

    /// 构造数值（NaN 规范化）。
    #[allow(non_snake_case)]
    #[must_use]
    #[inline]
    pub fn Number(n: f64) -> Self {
        if n.is_nan() {
            Self(NAN_CANONICAL)
        } else {
            Self(n.to_bits())
        }
    }

    /// 构造堆对象引用。
    #[allow(non_snake_case)]
    #[must_use]
    #[inline]
    pub fn Object(r: ObjectRef) -> Self {
        Self(TAG_PREFIX | TAG_OBJECT | ((u64::from(r.0)) << 8))
    }

    /// 原始机器字（JIT 边界用）。
    #[must_use]
    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// 从原始机器字还原（调用方保证来源合法，如 JIT 返回值）。
    #[must_use]
    #[inline]
    pub const fn from_bits(b: u64) -> Self {
        Self(b)
    }

    /// 是否为数值。
    #[must_use]
    #[inline]
    pub const fn is_number(&self) -> bool {
        (self.0 & TAG_MASK) != TAG_PREFIX
    }

    /// 是否为堆对象引用。
    #[must_use]
    #[inline]
    pub const fn is_object(&self) -> bool {
        (self.0 & TAG_MASK) == TAG_PREFIX && (self.0 & 0xFF) == TAG_OBJECT
    }

    /// 是否为布尔。
    #[must_use]
    #[inline]
    pub const fn is_boolean(&self) -> bool {
        (self.0 & TAG_MASK) == TAG_PREFIX && matches!(self.0 & 0xFF, TAG_FALSE | TAG_TRUE)
    }

    /// 是否为 undefined。
    #[must_use]
    #[inline]
    pub const fn is_undefined(&self) -> bool {
        self.0 == Self::Undefined.0
    }

    /// 是否为 null。
    #[must_use]
    #[inline]
    pub const fn is_null(&self) -> bool {
        self.0 == Self::Null.0
    }

    /// 数值出盒（非数值返回 None）。
    #[must_use]
    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        if self.is_number() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    /// 堆对象句柄出盒。
    #[must_use]
    #[inline]
    pub fn as_object(&self) -> Option<ObjectRef> {
        if self.is_object() {
            Some(ObjectRef(((self.0 >> 8) & 0xFFFF_FFFF) as u32))
        } else {
            None
        }
    }

    /// 布尔出盒。
    #[must_use]
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_boolean() {
            Some((self.0 & 0xFF) == TAG_TRUE)
        } else {
            None
        }
    }
}

/// 值类别判定（`match v.kind()` 形态的迁移辅助）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// undefined
    Undefined,
    /// null
    Null,
    /// 布尔
    Boolean,
    /// 数值
    Number,
    /// 堆对象引用
    Object,
}

impl Value {
    /// 值类别。
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> ValueKind {
        if self.is_number() {
            ValueKind::Number
        } else if self.is_object() {
            ValueKind::Object
        } else {
            match self.0 & 0xFF {
                TAG_NULL => ValueKind::Null,
                TAG_TRUE | TAG_FALSE => ValueKind::Boolean,
                _ => ValueKind::Undefined,
            }
        }
    }
}

/// 值的**模式匹配镜像**：旧枚举形态的 match 迁移辅助。
///
/// `Value` 本体为 NaN-box 机器字（无法被解构模式匹配），需要按变体分支时
/// 先经 `.case()` 转为本枚举——变体名与旧 `Value` 完全一致，逐位语义等价。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueCase {
    /// undefined
    Undefined,
    /// null
    Null,
    /// 布尔
    Boolean(bool),
    /// 数值
    Number(f64),
    /// 堆对象引用
    Object(ObjectRef),
}

impl From<&Value> for ValueCase {
    fn from(v: &Value) -> Self {
        match v.kind() {
            ValueKind::Undefined => Self::Undefined,
            ValueKind::Null => Self::Null,
            ValueKind::Boolean => Self::Boolean(v.as_bool().unwrap_or(false)),
            ValueKind::Number => Self::Number(v.as_number().unwrap_or(f64::NAN)),
            ValueKind::Object => Self::Object(v.as_object().unwrap_or(ObjectRef(u32::MAX))),
        }
    }
}

impl From<Value> for ValueCase {
    fn from(v: Value) -> Self {
        Self::from(&v)
    }
}

impl From<ObjectRef> for ValueCase {
    fn from(r: ObjectRef) -> Self {
        ValueCase::Object(r)
    }
}

impl From<ValueCase> for Value {
    fn from(c: ValueCase) -> Self {
        match c {
            ValueCase::Undefined => Self::Undefined,
            ValueCase::Null => Self::Null,
            ValueCase::Boolean(b) => Self::Boolean(b),
            ValueCase::Number(n) => Self::Number(n),
            ValueCase::Object(r) => Self::Object(r),
        }
    }
}

impl Value {
    /// 解构为模式匹配镜像（`match v.case() { ValueCase::Object(r) => … }`）。
    #[must_use]
    #[inline]
    pub fn case(&self) -> ValueCase {
        ValueCase::from(self)
    }
}

// M6.2 验收锚点：Value 必须恰为 8 字节机器字（编译期强制）
const _: () = assert!(std::mem::size_of::<Value>() == 8);

// 真值判定位于 `ops::to_boolean(val, heap)` / `Vm::truthy(val)`：
// 字符串是堆对象，空字符串必须为 falsy，判定需要堆访问。
impl From<Value> for aluka_core::Value {
    fn from(val: Value) -> Self {
        match val.kind() {
            ValueKind::Undefined => Self::Undefined,
            ValueKind::Null => Self::Null,
            ValueKind::Boolean => Self::Boolean(val.as_bool().unwrap_or(false)),
            ValueKind::Number => Self::Number(val.as_number().unwrap_or(f64::NAN)),
            ValueKind::Object => Self::Object(val.as_object().unwrap_or(ObjectRef(u32::MAX))),
        }
    }
}

/// 共享变量上值（Upvalue）句柄。
#[derive(Debug, Clone)]
pub struct Upvalue(pub Rc<RefCell<Value>>);
