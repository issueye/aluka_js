//! 词法分析：源码 → token 流。
//!
//! 支持 ECMAScript / TypeScript 关键字、多字符运算符、字符串转义、注释跳过。

/// Token 细分类别。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// 词法错误（未终止的多行注释等）：解析器按意外 token 报 SyntaxError
    LexError(String),
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
    /// 待发的词法错误（未终止多行注释等）：next_token 优先发出
    pending_lex_error: Option<String>,
}

/// 进制字符 → 进制基数（BigInt 校验用）。
fn radix_from_char(c: u8) -> u32 {
    match c {
        b'x' | b'X' => 16,
        b'b' | b'B' => 2,
        _ => 8,
    }
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

/// 从 `bytes[pos..]` 读取恰好 `n` 位十六进制数字，成功时推进 `pos` 返回数值。
/// 数字不足或不合法返回 `None`（`pos` 还原到调用前位置）。
fn read_hex_units(bytes: &[u8], pos: &mut usize, n: usize) -> Option<u32> {
    let start = *pos;
    let mut v = 0u32;
    for _ in 0..n {
        let Some(d) = (bytes.get(*pos).copied()? as char).to_digit(16) else {
            *pos = start; // 中途失败：还原（此前已推进的位不算数）
            return None;
        };
        v = v.saturating_mul(16).saturating_add(d);
        *pos += 1;
    }
    debug_assert!(*pos - start == n);
    Some(v)
}

impl<'src> Lexer<'src> {
    /// 在源码上创建分析器。
    #[must_use]
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            pos: 0,
            division_possible: false,
            pending_lex_error: None,
        }
    }

    /// 跳过空白字符与注释（单行 // 与多行 /* */）。
    fn skip_whitespace_and_comments(&mut self) {
        let bytes = self.src.as_bytes();
        // hashbang（`#!...` 仅允许源码首）：标准语法，此前被 `#`+`!` 误析
        if self.pos == 0 && bytes.len() >= 2 && bytes[0] == b'#' && bytes[1] == b'!' {
            while self.pos < bytes.len() && bytes[self.pos] != b'\n' {
                self.pos += 1;
            }
        }
        while self.pos < bytes.len() {
            // 空白字符
            if bytes[self.pos].is_ascii_whitespace() || bytes[self.pos] == 0x0B {
                // 0x0B = <VT> 垂直制表：规范 WhiteSpace（Rust 的
                // is_ascii_whitespace 不含它——S11.6.1 语料实测暴露）
                self.pos += 1;
                continue;
            }
            // U+2028（行分隔符）/ U+2029（段分隔符）：规范行终结符，
            // 同空白字符处理（M7.2 语料：`1 +1` 等 ~85 例依赖）
            if bytes[self.pos] == 0xE2
                && self.pos + 2 < bytes.len()
                && bytes[self.pos + 1] == 0x80
                && matches!(bytes[self.pos + 2], 0xA8 | 0xA9)
            {
                self.pos += 3;
                continue;
            }
            // Unicode 空白（规范 WhiteSpace 的 USP 面）：NBSP (C2 A0)、
            // ZWNBSP (EF BB BF)、U+1680/U+2000..200A/U+202F/U+205F/U+3000
            //（多字节 UTF-8 前缀 E1/E2/E3 起始的三字节形态）
            if bytes[self.pos] == 0xC2 && self.pos + 1 < bytes.len() && bytes[self.pos + 1] == 0xA0
            {
                self.pos += 2;
                continue;
            }
            if bytes[self.pos] == 0xEF
                && self.pos + 2 < bytes.len()
                && bytes[self.pos + 1] == 0xBB
                && bytes[self.pos + 2] == 0xBF
            {
                self.pos += 3;
                continue;
            }
            if matches!(bytes[self.pos], 0xE1..=0xE3)
                && self.pos + 2 < bytes.len()
                && matches!(
                    u32::from_le_bytes([0, bytes[self.pos + 1], bytes[self.pos + 2], 0]),
                    _
                )
            {
                // 三字节形态统一按码点解码后判 USP
                let cp = ((u32::from(bytes[self.pos] & 0x0F)) << 12)
                    | ((u32::from(bytes[self.pos + 1] & 0x3F)) << 6)
                    | (u32::from(bytes[self.pos + 2] & 0x3F));
                if matches!(cp, 0x1680 | 0x2000..=0x3000) {
                    self.pos += 3;
                    continue;
                }
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
                } else {
                    // 未终止的多行注释：规范为 SyntaxError
                    //（M7.2 语料暴露：`/*CHECK#1/` 被静默吞到 EOF）
                    self.pending_lex_error = Some("未终止的多行注释".to_owned());
                    self.pos = bytes.len();
                }
                continue;
            }
            break;
        }
    }

    /// 取下一个 token；输入耗尽后恒返回 [`TokenKind::Eof`]。
    pub fn next_token(&mut self) -> Token {
        if let Some(msg) = self.pending_lex_error.take() {
            let start = self.pos;
            self.pos = self.src.len();
            return Token {
                kind: TokenKind::LexError(msg),
                text: self.src[start..].to_owned(),
                start,
            };
        }

        let token = self.next_token_inner();
        // inner 在词末才探到未终止注释（pending 置位后返回 Eof）——
        // 此处拦截，把 Eof 换成 LexError 让解析器判死
        if token.kind == TokenKind::Eof
            && let Some(msg) = self.pending_lex_error.take()
        {
            let start = self.src.len().saturating_sub(1);
            return Token {
                kind: TokenKind::LexError(msg),
                text: self.src[start..].to_owned(),
                start,
            };
        }
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
                    let esc_start = self.pos;
                    current_raw.push('\\');
                    current_raw.push(bytes[self.pos] as char);
                    match bytes[self.pos] {
                        b'n' => current_quasi.push('\n'),
                        b't' => current_quasi.push('\t'),
                        b'r' => current_quasi.push('\r'),
                        b'\\' => current_quasi.push('\\'),
                        b'`' => current_quasi.push('`'),
                        b'$' => current_quasi.push('$'),
                        b'0' => current_quasi.push('\0'),
                        b'x' => {
                            // 跳过 'x' 再读 2 位十六进制；两路径都 continue
                            // （pos 已消费完整序列，跳过末尾 +1）
                            self.pos += 1;
                            match read_hex_units(bytes, &mut self.pos, 2) {
                                Some(v) => {
                                    current_quasi.push(char::from_u32(v).unwrap_or('\u{FFFD}'))
                                }
                                None => current_quasi.push('x'),
                            }
                            current_raw.push_str(&self.src[esc_start + 1..self.pos]);
                            continue;
                        }
                        b'u' => {
                            // 跳过 'u' 再读 4 位（或 \u{...} 花括号形式）；
                            // 各路径 pos 指向序列后，continue 跳过末尾 +1
                            self.pos += 1;
                            if bytes.get(self.pos) == Some(&b'{') {
                                let mut end = self.pos + 1;
                                let mut v = 0u32;
                                let mut valid = false;
                                while end < bytes.len() && bytes[end] != b'}' {
                                    let Some(d) = (bytes[end] as char).to_digit(16) else {
                                        break;
                                    };
                                    v = v.saturating_mul(16).saturating_add(d);
                                    valid = true;
                                    end += 1;
                                }
                                if valid && end < bytes.len() && bytes[end] == b'}' {
                                    current_quasi.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                                    self.pos = end + 1; // 越过 '}'
                                } else {
                                    // 非法 \u{...：'u' 按字面，'{' 留给普通字符路径
                                    current_quasi.push('u');
                                }
                            } else {
                                match read_hex_units(bytes, &mut self.pos, 4) {
                                    Some(v @ (0xD800..=0xDBFF)) => {
                                        if bytes.get(self.pos) == Some(&b'\\')
                                            && bytes.get(self.pos + 1) == Some(&b'u')
                                        {
                                            let mut probe = self.pos + 2;
                                            if let Some(lo) = read_hex_units(bytes, &mut probe, 4) {
                                                if (0xDC00..=0xDFFF).contains(&lo) {
                                                    let cp = 0x1_0000
                                                        + ((v - 0xD800) << 10)
                                                        + (lo - 0xDC00);
                                                    current_quasi.push(
                                                        char::from_u32(cp).unwrap_or('\u{FFFD}'),
                                                    );
                                                    self.pos = probe;
                                                } else {
                                                    current_quasi.push('\u{FFFD}');
                                                }
                                            } else {
                                                current_quasi.push('\u{FFFD}');
                                            }
                                        } else {
                                            current_quasi.push('\u{FFFD}');
                                        }
                                    }
                                    Some(v) => {
                                        current_quasi.push(char::from_u32(v).unwrap_or('\u{FFFD}'))
                                    }
                                    None => current_quasi.push('u'),
                                }
                            }
                            current_raw.push_str(&self.src[esc_start + 1..self.pos]);
                            continue;
                        }
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
                        b'0' => s.push('\0'),
                        b'x' => {
                            // 跳过 'x' 再读 2 位十六进制。成功时 pos 指向序列后；
                            // 失败时 pos 停在 'x' 后、按字面输出 'x'。
                            // 两路径都 continue：跳过循环体末尾的 +1（已经消费到
                            // 序列末尾之后，多跳会吞掉闭合引号）。
                            self.pos += 1;
                            match read_hex_units(bytes, &mut self.pos, 2) {
                                Some(v) => s.push(char::from_u32(v).unwrap_or('\u{FFFD}')),
                                None => s.push('x'),
                            }
                            continue;
                        }
                        b'u' => {
                            // 跳过 'u' 再读 4 位（或 \u{...} 花括号形式）。
                            // 各路径统一保证 pos 指向转义序列后的第一个字符，
                            // 再 continue 跳过循环体末尾的 +1。
                            self.pos += 1;
                            if bytes.get(self.pos) == Some(&b'{') {
                                let mut end = self.pos + 1;
                                let mut v = 0u32;
                                let mut valid = false;
                                while end < bytes.len() && bytes[end] != b'}' {
                                    let Some(d) = (bytes[end] as char).to_digit(16) else {
                                        break;
                                    };
                                    v = v.saturating_mul(16).saturating_add(d);
                                    valid = true;
                                    end += 1;
                                }
                                if valid && end < bytes.len() && bytes[end] == b'}' {
                                    s.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                                    self.pos = end + 1; // 越过 '}'
                                } else {
                                    // 非法 \u{...：'u' 按字面，'{' 留给普通字符路径
                                    s.push('u');
                                }
                            } else {
                                let hi = read_hex_units(bytes, &mut self.pos, 4);
                                match hi {
                                    Some(v @ (0xD800..=0xDBFF)) => {
                                        // 高位代理：紧跟 \uDC00..\uDFFF 时组合为码点
                                        // （孤立代理按 U+FFFD 替换，Rust 字符串无法承载）
                                        if bytes.get(self.pos) == Some(&b'\\')
                                            && bytes.get(self.pos + 1) == Some(&b'u')
                                        {
                                            let mut probe = self.pos + 2;
                                            if let Some(lo) = read_hex_units(bytes, &mut probe, 4) {
                                                if (0xDC00..=0xDFFF).contains(&lo) {
                                                    let cp = 0x1_0000
                                                        + ((v - 0xD800) << 10)
                                                        + (lo - 0xDC00);
                                                    s.push(
                                                        char::from_u32(cp).unwrap_or('\u{FFFD}'),
                                                    );
                                                    self.pos = probe;
                                                } else {
                                                    s.push('\u{FFFD}');
                                                }
                                            } else {
                                                s.push('\u{FFFD}');
                                            }
                                        } else {
                                            s.push('\u{FFFD}');
                                        }
                                    }
                                    Some(v) => s.push(char::from_u32(v).unwrap_or('\u{FFFD}')),
                                    None => s.push('u'),
                                }
                            }
                            continue;
                        }
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
                // BigInt 后缀（`0xFFn`）：进制数字循环按 alphanumeric 吞字，
                // 后缀 `n`/`N` 落在 raw_digits 尾部（十六进制下 n 非法数字位，
                // 二/八进制同）——剥后缀并以 `0x` 形态整串作 BigInt 载荷；
                // VM 物化 Constant::BigInt 时经 BigNat 归一化十进制
                //（M7.2 修复：此前 i64 溢出得 0，大十六进制 BigInt 全损坏）
                let bigint_payload = raw_digits
                    .strip_suffix('n')
                    .or_else(|| raw_digits.strip_suffix('N'))
                    .map(|digits| (digits, format!("0{}{digits}", radix_char as char)));
                if let Some((digits, payload)) = bigint_payload {
                    // 合法性（M7.2 语料）：数字位必须属于该进制；分隔符不得
                    // 位于首尾或连续
                    if digits
                        .chars()
                        .any(|ch| ch != '_' && ch.to_digit(radix_from_char(radix_char)).is_none())
                    {
                        return Token {
                            kind: TokenKind::LexError("BigInt 字面量含无效进制数字".to_owned()),
                            text: self.src[start..self.pos].to_owned(),
                            start,
                        };
                    }
                    if digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
                        return Token {
                            kind: TokenKind::LexError(
                                "BigInt 字面量的数字分隔符位置非法".to_owned(),
                            ),
                            text: self.src[start..self.pos].to_owned(),
                            start,
                        };
                    }
                    return Token {
                        kind: TokenKind::BigInt(payload),
                        text: self.src[start..self.pos].to_owned(),
                        start,
                    };
                }
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
            // 科学计数法指数段（`1e21` / `1.5e-7`；实测缺失导致 `1e` 被拆成
            // 标识符，`String(1e21)` 直接解析失败）
            if self.pos < bytes.len() && matches!(bytes[self.pos], b'e' | b'E') {
                let mut ahead = self.pos + 1;
                if ahead < bytes.len() && matches!(bytes[ahead], b'+' | b'-') {
                    ahead += 1;
                }
                if ahead < bytes.len() && bytes[ahead].is_ascii_digit() {
                    self.pos = ahead;
                    while self.pos < bytes.len() && bytes[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
            }
            if self.pos < bytes.len() && bytes[self.pos] == b'n' {
                let raw_with_sep: String = self.src[start..self.pos].to_owned();
                let raw_digits: String = raw_with_sep.chars().filter(|&c| c != '_').collect();
                self.pos += 1;
                // 合法性（M7.2 语料）：不得含指数；传统八进制形态（前导 0
                // 且多位）不得作 BigInt；分隔符不得居首/居尾/连续
                if raw_digits.contains(['e', 'E']) {
                    return Token {
                        kind: TokenKind::LexError("BigInt 字面量不支持指数".to_owned()),
                        text: self.src[start..self.pos].to_owned(),
                        start,
                    };
                }
                if raw_digits.len() > 1 && raw_digits.starts_with('0') {
                    return Token {
                        kind: TokenKind::LexError("传统八进制形态不能作 BigInt 字面量".to_owned()),
                        text: self.src[start..self.pos].to_owned(),
                        start,
                    };
                }
                if raw_with_sep.starts_with('_')
                    || raw_with_sep.ends_with('_')
                    || raw_with_sep.contains("__")
                {
                    return Token {
                        kind: TokenKind::LexError("BigInt 字面量的数字分隔符位置非法".to_owned()),
                        text: self.src[start..self.pos].to_owned(),
                        start,
                    };
                }
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
            "...", ">>>=", "===", "!==", ">>>", "**=", "<<=", ">>=", "&&=", "||=", "??=", "&=",
            "|=", "^=", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "**", "<<",
            ">>", "+=", "-=", "*=", "/=", "%=", "=>",
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

    #[test]
    fn decodes_unicode_escapes_in_strings() {
        // \uFFFD → U+FFFD 替换符
        let mut lexer = Lexer::new("'\\uFFFD'");
        assert_eq!(
            lexer.next_token().kind,
            TokenKind::String("\u{FFFD}".to_owned())
        );
        // \u{1F600} → 😀
        let mut lexer = Lexer::new("'\\u{1F600}'");
        assert_eq!(
            lexer.next_token().kind,
            TokenKind::String("\u{1F600}".to_owned())
        );
        // 代理对 \uD83D\uDE00 → 😀
        let mut lexer = Lexer::new("'\\uD83D\\uDE00'");
        assert_eq!(
            lexer.next_token().kind,
            TokenKind::String("\u{1F600}".to_owned())
        );
        // \x41 → 'A'
        let mut lexer = Lexer::new("'\\x41'");
        assert_eq!(lexer.next_token().kind, TokenKind::String("A".to_owned()));
        // 模板字符串内的 \uFFFD
        let mut lexer = Lexer::new("`\\uFFFD`");
        let t = lexer.next_token();
        match t.kind {
            TokenKind::TemplateLiteral { quasis, .. } => {
                assert_eq!(quasis, vec!["\u{FFFD}".to_owned()]);
            }
            other => panic!("expected template literal, got {other:?}"),
        }
    }
}
