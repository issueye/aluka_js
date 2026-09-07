//! 虚拟机运行时值表示与上值句柄定义。

use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::rc::Rc;

/// 运行时求值结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// 未定义值
    Undefined,
    /// 空值
    Null,
    /// 布尔值
    Boolean(bool),
    /// 双精度浮点数值
    Number(f64),
    /// 堆对象引用句柄
    Object(ObjectRef),
}

impl Value {
    /// 判断是否为真值（Truthy）。
    #[must_use]
    pub fn is_truthy(self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(b) => b,
            Self::Number(n) => n != 0.0 && !n.is_nan(),
            Self::Object(_) => true,
        }
    }
}

impl From<Value> for aluka_core::Value {
    fn from(val: Value) -> Self {
        match val {
            Value::Undefined => Self::Undefined,
            Value::Null => Self::Null,
            Value::Boolean(b) => Self::Boolean(b),
            Value::Number(n) => Self::Number(n),
            Value::Object(r) => Self::Object(r),
        }
    }
}

/// 共享变量上值（Upvalue）句柄。
#[derive(Debug, Clone)]
pub struct Upvalue(pub Rc<RefCell<Value>>);
