//! AST → 字节码编译器门面。
//!
//! 提供语法树遍历生成、跳转回填及函数模板导出。

/// 依赖闭包镜像构建（`alukac build` / `aluka build` 共用）
pub mod build;
/// 语法树遍历与指令生成
pub mod codegen;
/// S-expression 领域特定语言（DSL）编译器
pub mod dsl;
/// 编译期错误类型
pub mod error;
/// 操作数栈峰值计算
pub mod max_stack;
/// 模块级函数与类模板编译器
pub mod module;
/// AST 与指令级优化器
pub mod opt;
/// 词法作用域分析与符号管理
pub mod scope;
/// 源码单元编译与流水线控制器
pub mod source_unit;

pub use codegen::{backpatch_jump, compile, emit_jump};
pub use dsl::{DslCompiler, compile_dsl_source};
pub use error::CompileError;
pub use max_stack::compute_max_stack;
pub use module::{
    ModuleCompiler, compile_esm_module, compile_module, compile_module_with_coverage,
};
pub use opt::{optimize_ast, optimize_jumps};
pub use scope::{CompiledUnit, ResolvedSymbol, Scope, ScopeKind, ScopeTree};
pub use source_unit::{compile_source_unit, parse_json_to_expr};
