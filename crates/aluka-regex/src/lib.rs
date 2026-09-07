//! RegExp 引擎。
//!
//! 两条路径：能翻译成线性时间自动机的模式走快路径；需要回溯的特性
//! （前后行断言、反向引用）走自研回溯引擎，并带**预算护栏**。
//!
//! # 两条必须守住的语义
//!
//! - **索引以 UTF-16 为单位**：JS 可见的 `lastIndex`、capture 偏移、
//!   `replace`/`split` 结果都按 code unit 计（`u` 模式按 code point）。
//!   Rust 的 `str` 是 UTF-8，两套索引之间的换算是本 crate 的核心责任。
//! - **预算耗尽必须报错**：回溯超限要显式抛出，绝不能折叠成"无匹配"——
//!   后者会把死循环伪装成正常结果。Go 版为此专门立了规矩（见 `AGENTS.md`
//!   的正则差分与预算段）。

/// 匹配失败或无法执行的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegexError {
    /// 模式语法错误
    Syntax(String),
    /// 回溯预算耗尽（必须上抛，不可当作无匹配）
    BacktrackLimit,
}

impl std::fmt::Display for RegexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegexError::Syntax(msg) => write!(f, "invalid regular expression: {msg}"),
            RegexError::BacktrackLimit => write!(f, "regular expression backtrack limit exceeded"),
        }
    }
}

impl std::error::Error for RegexError {}

mod engine;
mod parser;

pub use engine::{MatchResult, Regex};

/// 一次成功匹配的位置，索引以 UTF-16 code unit 计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// 起始（含）
    pub start: usize,
    /// 结束（不含）
    pub end: usize,
}

/// 把 UTF-8 字节偏移换算成 UTF-16 code unit 偏移。
///
/// 这是 JS 可见索引的换算基石：BMP 内字符占 1 个 code unit，星面字符
/// （U+10000 及以上）占 2 个。
///
/// # Panics
///
/// `byte_offset` 不在字符边界上时 panic——调用方应传入由 `char_indices`
/// 等安全途径得到的偏移。
#[must_use]
pub fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    assert!(
        text.is_char_boundary(byte_offset),
        "byte offset must fall on a char boundary"
    );
    text[..byte_offset].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offset_counts_bmp_chars_as_one_unit() {
        let text = "abc";
        assert_eq!(utf16_offset(text, 0), 0);
        assert_eq!(utf16_offset(text, 3), 3);
    }

    #[test]
    fn utf16_offset_counts_astral_chars_as_two_units() {
        // U+1F600 占 4 个 UTF-8 字节、2 个 UTF-16 code unit。
        let text = "a\u{1F600}b";
        assert_eq!(utf16_offset(text, 1), 1);
        assert_eq!(utf16_offset(text, 5), 3);
        assert_eq!(utf16_offset(text, text.len()), 4);
    }

    #[test]
    fn backtrack_limit_is_distinguishable_from_syntax_error() {
        let limit = RegexError::BacktrackLimit;
        let syntax = RegexError::Syntax("unbalanced paren".to_owned());
        assert_ne!(limit, syntax);
        assert!(limit.to_string().contains("backtrack"));
    }
}
