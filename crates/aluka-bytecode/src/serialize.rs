//! 字节码磁盘缓存的编解码。
//!
//! 缓存以 [`FORMAT_VERSION`] 为门禁：**改动指令布局、常量编码或编译器
//! 输出都必须递增它**，否则旧缓存会被误读为新格式。Go 版把这条写进了
//! `AGENTS.md` 的硬约束（`bytecode/serialize.go` 的 FormatVersion），
//! Rust 版沿用同一纪律。

/// 缓存格式版本。语义见模块文档：布局变更即递增。
pub const FORMAT_VERSION: u32 = 1;

// 版本号是编译期常量：用 const 断言让"版本必须为正"在编译期成立，
// 而不是留到运行测试才发现。
const _: () = assert!(FORMAT_VERSION >= 1);
