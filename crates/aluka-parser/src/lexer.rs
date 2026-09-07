//! 词法分析：源码 → token 流。
//!
//! 支持 ECMAScript / TypeScript 关键字、多字符运算符、字符串转义、注释跳过。

/// Token 细分类别。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// 数值字面量
    Number(f64),
    /// 大整数字面量（如 123n）
    BigInt(String),
    /// 字符串字面量
    String(String),
    /// 标识符
    Ident(String),
    /// 关键字
    Keyword(String),
    /// 标点与运算符
    Punct(String),
    /// 正则字面量（模式原文 + 标志；`/ab\/c/gi` 整体成词，避免体内
    /// `//`、`/*` 被误当注释/除法）
    RegexLiteral {
        /// 模式原文（不含定界 `/`）
        pattern: String,
        /// 标志字母
        flags: String,
    },
    /// 模板字符串字面量（静态片段列表与表达式源码列表）
    TemplateLiteral {
        /// 静态片段列表
        quasis: Vec<String>,
        /// 各片段的原始文本（转义保留原文）
        raw_quasis: Vec<String>,
        /// 表达式源码子串列表
        raw_exprs: Vec<String>,
    },
    /// 输入结束
    Eof,
}

/// 一个 token 及其在源码中的起始字节偏移。
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// 类别
    pub kind: TokenKind,
    /// 原始文本
    pub text: String,
    /// 起始字节偏移
    pub start: usize,
}

impl Token {
    /// 判定是否为指定的标点符号
    #[must_use]
    pub fn is_punct(&self, p: &str) -> bool {
        matches!(&self.kind, TokenKind::Punct(s) if s == p)
    }
}

/// 词法分析器。
#[derive(Debug)]
pub struct Lexer<'src> {
    src: &'src str,
    pos: usize,
    /// 上一个 token 之后是否允许「除法」（false 时 `/` 按正则字面量分词）
    division_possible: bool,
}

const KEYWORDS: &[&str] = &[
    "let",
    "const",
    "var",
    "function",
    "class",
    "extends",
    "if",
    "else",
    "while",
    "for",
    "break",
    "continue",
    "return",
    "try",
    "catch",
    "finally",
    "throw",
    "new",
    "this",
    "super",
    "true",
    "false",
    "null",
    "interface",
    "as",
    "typeof",
    "delete",
    "void",
    "in",
    "instanceof",
    "yield",
    "await",
    "async",
    "switch",
    "case",
    "default",
    "do",
    "import",
    "export",
    "from",
];

impl<'src> Lexer<'src> {
    /// 在源码上创建分析器。
    #[must_use]
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            pos: 0,
            division_possible: false,
        }
    }

    /// 跳过空白字符与注释（单行 // 与多行 /* */）。
    fn skip_whitespace_and_comments(&mut self) {
        let bytes = self.src.as_bytes();
        while self.pos < bytes.len() {
            // 空白字符
            if bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
                continue;
            }
            // 单行注释 //
            if self.pos + 1 < bytes.len() && bytes[self.pos] == b'/' && bytes[self.pos + 1] == b'/'
            {
                self.pos += 2;
                while self.pos < bytes.len() && bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            // 多行注释 /* */
            if self.pos + 1 < bytes.len() && bytes[self.pos] == b'/' && bytes[self.pos + 1] == b'*'
            {
                self.pos += 2;
                while self.pos + 1 < bytes.len()
                    && !(bytes[self.pos] == b'*' && bytes[self.pos + 1] == b'/')
                {
                    self.pos += 1;
                }
                if self.pos + 1 < bytes.len() {
                    self.pos += 2; // 跳过 */
                }
                continue;
            }
            break;
        }
    }

    /// 取下一个 token；输入耗尽后恒返回 [`TokenKind::Eof`]。
    pub fn next_token(&mut self) -> Token {
        let token = self.next_token_inner();
        self.division_possible = matches!(
            &token.kind,
            TokenKind::Ident(_)
                | TokenKind::Number(_)
                | TokenKind::BigInt(_)
                | TokenKind::String(_)
                | TokenKind::TemplateLiteral { .. }
                | TokenKind::RegexLiteral { .. }
        ) || matches!(&token.kind, TokenKind::Punct(p) if p == ")" || p == "]" || p == "}")
            || matches!(
                &token.kind,
                TokenKind::Keyword(k)
                    if matches!(k.as_str(), "this" | "super" | "true" | "false" | "null")
            );
        token
    }

    fn next_token_inner(&mut self) -> Token {
        self.skip_whitespace_and_comments();
        let bytes = self.src.as_bytes();
        if self.pos >= bytes.len() {
            return Token {
                kind: TokenKind::Eof,
                text: String::new(),
                start: self.pos,
            };
        }

        let start = self.pos;
        let first = bytes[self.pos];

        // 1. 模板字符串字面量 (`...`)
        if first == b'`' {
            self.pos += 1;
            let mut quasis = Vec::new();
            // 各段 quasi 的原始文本（转义保留原文，raw 属性用）
            let mut raw_quasis = Vec::new();
            let mut raw_exprs = Vec::new();
            let mut current_quasi = String::new();
            let mut current_raw = String::new();

            while self.pos < bytes.len() && bytes[self.pos] != b'`' {
                if bytes[self.pos] == b'$'
                    && self.pos + 1 < bytes.len()
                    && bytes[self.pos + 1] == b'{'
                {
                    self.pos += 2;
                    quasis.push(std::mem::take(&mut current_quasi));
                    raw_quasis.push(std::mem::take(&mut current_raw));

                    let expr_start = self.pos;
                    let mut brace_depth = 1usize;
                    while self.pos < bytes.len() && brace_depth > 0 {
                        let b = bytes[self.pos];
                        if b == b'{' {
                            brace_depth += 1;
                            self.pos += 1;
                        } else if b == b'}' {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                break;
                            }
                            self.pos += 1;
                        } else if b == b'"' || b == b'\'' || b == b'`' {
                            let quote = b;
                            self.pos += 1;
                            while self.pos < bytes.len() && bytes[self.pos] != quote {
                                if bytes[self.pos] == b'\\' && self.pos + 1 < bytes.len() {
                                    self.pos += 2;
                                } else {
                                    self.pos += 1;
                                }
                            }
                            if self.pos < bytes.len() && bytes[self.pos] == quote {
                                self.pos += 1;
                            }
                        } else if b == b'\\' && self.pos + 1 < bytes.len() {
                            self.pos += 2;
                        } else {
                            self.pos += 1;
                        }
                    }
                    let expr_end = self.pos;
                    if self.pos < bytes.len() && bytes[self.pos] == b'}' {
                        self.pos += 1;
                    }
                    raw_exprs.push(self.src[expr_start..expr_end].trim().to_owned());
                } else if bytes[self.pos] == b'\\' && self.pos + 1 < bytes.len() {
                    self.pos += 1;
                    // raw 文本保留转义原文（反斜杠 + 转义字符）
                    current_raw.push('\\');
                    current_raw.push(bytes[self.pos] as char);
                    match bytes[self.pos] {
                        b'n' => current_quasi.push('\n'),
                        b't' => current_quasi.push('\t'),
                        b'r' => current_quasi.push('\r'),
                        b'\\' => current_quasi.push('\\'),
                        b'`' => current_quasi.push('`'),
                        b'$' => current_quasi.push('$'),
                        other => current_quasi.push(other as char),
                    }
                    self.pos += 1;
                } else {
                    // 多字节 UTF-8：按整字符入 quasi，按字符宽度推进
                    let ch = self.src[self.pos..].chars().next().unwrap_or('\u{FFFD}');
                    current_quasi.push(ch);
                    current_raw.push(ch);
                    self.pos += ch.len_utf8();
                }
            }

            if self.pos < bytes.len() && bytes[self.pos] == b'`' {
                self.pos += 1;
            }
            quasis.push(current_quasi);
            raw_quasis.push(current_raw);

            return Token {
                kind: TokenKind::TemplateLiteral {
                    quasis,
                    raw_quasis,
                    raw_exprs,
                },
                text: self.src[start..self.pos].to_owned(),
                start,
            };
        }

        // 2. 普通字符串字面量 ("..." 或 '...')
        if first == b'"' || first == b'\'' {
            let quote = first;
            self.pos += 1;
            let mut s = String::new();
            while self.pos < bytes.len() && bytes[self.pos] != quote {
                if bytes[self.pos] == b'\\' && self.pos + 1 < bytes.len() {
                    self.pos += 1;
                    match bytes[self.pos] {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'\\' => s.push('\\'),
                        b'"' => s.push('"'),
                        b'\'' => s.push('\''),
                        other => s.push(other as char),
                    }
                } else {
                    // 多字节 UTF-8：按整字符入串，按字符宽度推进（跳过共享步进）
                    let ch = self.src[self.pos..].chars().next().unwrap_or('�');
                    s.push(ch);
                    self.pos += ch.len_utf8();
                    continue;
                }
                self.pos += 1;
            }
            if self.pos < bytes.len() && bytes[self.pos] == quote {
                self.pos += 1;
            }
            return Token {
                kind: TokenKind::String(s),
                text: self.src[start..self.pos].to_owned(),
                start,
            };
        }

        // 2. 数值字面量
        if first.is_ascii_digit() {
            // 0x 十六进制 / 0b 二进制 / 0o 八进制（真实包常用位掩码形态）
            if first == b'0'
                && self.pos + 1 < bytes.len()
                && matches!(bytes[self.pos + 1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
            {
                let radix_char = bytes[self.pos + 1];
                self.pos += 2;
                let start_digits = self.pos;
                while self.pos < bytes.len()
                    && (bytes[self.pos].is_ascii_alphanumeric() || bytes[self.pos] == b'_')
                {
                    self.pos += 1;
                }
                let raw_digits: String = self.src[start_digits..self.pos]
                    .chars()
                    .filter(|&c| c != '_')
                    .collect();
                let val = match radix_char {
                    b'x' | b'X' => i64::from_str_radix(&raw_digits, 16),
                    b'b' | b'B' => i64::from_str_radix(&raw_digits, 2),
                    _ => i64::from_str_radix(&raw_digits, 8),
                }
                .map(|v| v as f64)
                .unwrap_or(0.0);
                return Token {
                    kind: TokenKind::Number(val),
                    text: self.src[start..self.pos].to_owned(),
                    start,
                };
            }
            while self.pos < bytes.len()
                && (bytes[self.pos].is_ascii_digit()
                    || bytes[self.pos] == b'.'
                    || bytes[self.pos] == b'_')
            {
                self.pos += 1;
            }
            if self.pos < bytes.len() && bytes[self.pos] == b'n' {
                let raw_digits: String = self.src[start..self.pos]
                    .chars()
                    .filter(|&c| c != '_')
                    .collect();
                self.pos += 1;
                return Token {
                    kind: TokenKind::BigInt(raw_digits),
                    text: self.src[start..self.pos].to_owned(),
                    start,
                };
            }
            let raw_str: String = self.src[start..self.pos]
                .chars()
                .filter(|&c| c != '_')
                .collect();
            let val = raw_str.parse::<f64>().unwrap_or(0.0);
            return Token {
                kind: TokenKind::Number(val),
                text: self.src[start..self.pos].to_owned(),
                start,
            };
        }

        // 2.5 正则字面量：`/` 后不可能是除法时（前缀位置），整体成词
        if first == b'/'
            && !self.division_possible
            && !(self.pos + 1 < bytes.len() && bytes[self.pos + 1] == b'=')
        {
            let mut idx = self.pos + 1;
            let mut in_class = false;
            let mut closed = false;
            while idx < bytes.len() {
                let ch = bytes[idx];
                if ch == b'\n' || ch == b'\r' {
                    break;
                }
                if ch == b'\\' {
                    idx += 1;
                    if idx < bytes.len() {
                        idx += 1;
                    }
                    continue;
                }
                if ch == b'[' {
                    in_class = true;
                } else if ch == b']' {
                    in_class = false;
                } else if ch == b'/' && !in_class {
                    closed = true;
                    break;
                }
                idx += 1;
            }
            if closed {
                let pattern = self.src[self.pos + 1..idx].to_owned();
                idx += 1;
                let flags_start = idx;
                while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
                    idx += 1;
                }
                let flags = self.src[flags_start..idx].to_owned();
                self.pos = idx;
                return Token {
                    kind: TokenKind::RegexLiteral { pattern, flags },
                    text: self.src[start..self.pos].to_owned(),
                    start,
                };
            }
        }

        // 3. 标识符与关键字
        if first.is_ascii_alphabetic() || first == b'_' || first == b'$' {
            while self.pos < bytes.len()
                && (bytes[self.pos].is_ascii_alphanumeric()
                    || bytes[self.pos] == b'_'
                    || bytes[self.pos] == b'$')
            {
                self.pos += 1;
            }
            let text = self.src[start..self.pos].to_owned();
            let kind = if KEYWORDS.contains(&text.as_str()) {
                TokenKind::Keyword(text.clone())
            } else {
                TokenKind::Ident(text.clone())
            };
            return Token { kind, text, start };
        }

        // 4. 多字符运算符与标点
        let multi_puncts = &[
            "...", ">>>=", "===", "!==", ">>>", "**=", "<<=", ">>=", "&&=", "||=", "??=", "==",
            "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "**", "<<", ">>", "+=", "-=",
            "*=", "/=", "%=", "=>",
        ];

        for &mp in multi_puncts {
            if self.src[self.pos..].starts_with(mp) {
                self.pos += mp.len();
                return Token {
                    kind: TokenKind::Punct(mp.to_owned()),
                    text: mp.to_owned(),
                    start,
                };
            }
        }

        // 单字符标点或未知字符（按 Unicode 字符边界步进）
        if let Some(ch) = self.src[self.pos..].chars().next() {
            self.pos += ch.len_utf8();
        } else {
            self.pos += 1;
        }
        let text = self.src[start..self.pos].to_owned();
        Token {
            kind: TokenKind::Punct(text.clone()),
            text,
            start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_numbers_idents_and_puncts() {
        let mut lexer = Lexer::new("let x1 = 42; // 注释\n/* 多行 */ x1 === \"hello\";");
        let tokens: Vec<Token> = std::iter::from_fn(|| {
            let token = lexer.next_token();
            if token.kind == TokenKind::Eof {
                None
            } else {
                Some(token)
            }
        })
        .collect();

        assert_eq!(tokens[0].kind, TokenKind::Keyword("let".to_owned()));
        assert_eq!(tokens[1].kind, TokenKind::Ident("x1".to_owned()));
        assert_eq!(tokens[2].kind, TokenKind::Punct("=".to_owned()));
        assert_eq!(tokens[3].kind, TokenKind::Number(42.0));
        assert_eq!(tokens[4].kind, TokenKind::Punct(";".to_owned()));
        assert_eq!(tokens[5].kind, TokenKind::Ident("x1".to_owned()));
        assert_eq!(tokens[6].kind, TokenKind::Punct("===".to_owned()));
        assert_eq!(tokens[7].kind, TokenKind::String("hello".to_owned()));
        assert_eq!(tokens[8].kind, TokenKind::Punct(";".to_owned()));
    }

    #[test]
    fn reports_eof_after_input_is_consumed() {
        let mut lexer = Lexer::new("  \n\t // trailing comment\n ");
        assert_eq!(lexer.next_token().kind, TokenKind::Eof);
        assert_eq!(lexer.next_token().kind, TokenKind::Eof);
    }
}
