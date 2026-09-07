//! 字节码指令集、序列化与优化。
//!
//! 指令为定长四字节（1 字节 opcode + 3 字节操作数），与 Go 版一致，便于
//! 直接移植既有的优化 pass 与差分测试语料。
//!
//! # 元数据是单一事实来源
//!
//! 每条指令的栈效果登记在 [`Op::stack_effect`] 一处，反汇编、优化器与
//! 校验器都从它派生。Go 版把这条约定写进了 `AGENTS.md`（新增指令必须
//! 登记元数据）；Rust 版靠 `match` 穷尽性把它变成编译期强制。

pub mod aluc;
pub mod extras;
pub mod op;
pub mod serialize;
pub mod serializer;
pub mod verifier;

pub use aluc::{ALUC_CONTAINER_VERSION, ALUC_MAGIC};
pub use extras::{FuncHeaderExtras, read_all_func_header_extras};
pub use op::{Instr, Op, OperandKind, StackEffect};
pub use serialize::FORMAT_VERSION;
pub use verifier::{
    BytecodeModule, ClassMethod, ClassTemplate, Constant, FuncTemplate, TryEntry, UpvalueCapture,
    VerifyError,
};
