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

// 真值判定位于 `ops::to_boolean(val, heap)` / `Vm::truthy(val)`：
// 字符串是堆对象，空字符串必须为 falsy，判定需要堆访问。

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
