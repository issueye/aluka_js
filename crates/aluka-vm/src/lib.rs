//! Tier 0 字节码虚拟机解释器门面。
//!
//! 负责字节码解释循环执行、堆对象管理、调用帧与作用域隔离、类与原型链继承。

pub(crate) mod bigdec;
pub mod builtins;
pub mod call;
pub mod class;
pub mod eval;
pub mod exception;
pub mod gc;
pub mod generator;
pub mod heap;
pub mod interpreter;
pub mod iter;
pub mod jit_helpers;
pub mod jit_hot;
pub mod microtask;
pub mod modules;
pub mod ops;
pub mod prims;
pub mod property;
pub mod proxy;
pub mod symbol;

// TLS spike（T10 评估，仅测试编译）
#[cfg(test)]
mod tls_spike;
pub mod typed_array;
pub mod value;
pub mod worker;
mod worker_clone;

pub use eval::empty_eval_module;
pub use heap::HeapObject;
pub use interpreter::{Vm, VmError};
pub use jit_hot::JIT_HOT_THRESHOLD;
pub use value::{Upvalue, Value};
