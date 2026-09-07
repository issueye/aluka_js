//! 编译器错误类型定义。

/// 编译期发生的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// 语法节点不受支持或暂未实现
    UnsupportedSyntax(String),
    /// 操作数超出编码限制
    OperandOverflow(String),
    /// 源码单元处理或阶段错误
    SourceUnitError(String),
    /// JSON 源码解析错误
    JsonParseError(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSyntax(msg) => write!(f, "不支持的语法: {msg}"),
            Self::OperandOverflow(msg) => write!(f, "操作数溢出: {msg}"),
            Self::SourceUnitError(msg) => write!(f, "源码单元错误: {msg}"),
            Self::JsonParseError(msg) => write!(f, "JSON 解析错误: {msg}"),
        }
    }
}

impl From<aluka_parser::source_unit::SourceUnitError> for CompileError {
    fn from(err: aluka_parser::source_unit::SourceUnitError) -> Self {
        Self::SourceUnitError(err.to_string())
    }
}

impl std::error::Error for CompileError {}
