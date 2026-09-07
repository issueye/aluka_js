//! 词法与语法分析。
//!
//! 把源码切成 token 流，再规约成 [`ast`] 里的语法树。TypeScript 的类型
//! 注解在这一层被识别并标记，实际剥离发生在 `aluka-compiler`——这与 Go 版
//! 的分工一致（parser 认语法，compiler 决定是否发射代码）。

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod source_unit;

pub use ast::{
    ArrayPatternElem, ClassMethodDef, ExportDecl, ExportSpecifier, Expr, FunctionDef, ImportDecl,
    ImportSpecifier, ObjectProp, Program, PropKey, PropValue, Stmt, VarPattern,
};
pub use lexer::{Lexer, Token, TokenKind};
pub use parser::{Parser, parse};
pub use source_unit::{
    LanguageRegistry, ModuleKind, STAGE_BYTECODE_COMPILED, STAGE_BYTECODE_OPTIMIZED,
    STAGE_ESM_LOWERED, STAGE_MINIFIED, STAGE_PARSED, STAGE_SHAKEN, STAGE_TYPE_STRIPPED,
    STAGE_WRAPPED, SourceKind, SourceUnit, SourceUnitError, TransformStage, detect_source_kind,
    parse_file_unit, parse_source_unit,
};
