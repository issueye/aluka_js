//! 正则模式解析器：递归下降把模式字符串解析为 AST。
//!
//! 覆盖语法（Tier 0 子集）：字面量、`.`、字符类（范围/取反/类简写）、
//! `\d \D \w \W \s \S \b 转义`、量词 `* + ? {m} {m,} {m,n}`（贪婪/懒惰）、
//! 捕获组 `(...)` 与非捕获组 `(?:...)`、选择 `|`、锚点 `^ $`；
//! 标志 `i`（忽略大小写）、`g`、`m`。

use crate::RegexError;

/// 字符类成员。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClassItem {
    /// 单个字符
    Ch(char),
    /// 单个码点（可容纳代理区码点 `\uD800..=\uDFFF`——Rust `char` 无法表示，
    /// 由匹配器按 UTF-16 代理码元语义参与判定）
    Cp(u32),
    /// 码点范围（含两端；同上可覆盖代理区带）
    Range(u32, u32),
    /// `\d`
    Digit,
    /// `\D`
    NotDigit,
    /// `\w`
    Word,
    /// `\W`
    NotWord,
    /// `\s`
    Space,
    /// `\S`
    NotSpace,
}

/// 正则 AST 节点。
#[derive(Debug, Clone)]
pub(crate) enum Node {
    /// 字面量字符
    Char(char),
    /// `.`（任意字符，不含行终止符——语料未涉及，暂按任意处理）
    Any,
    /// 字符类
    Class {
        /// 是否取反 `[^...]`
        negated: bool,
        /// 类成员列表
        items: Vec<ClassItem>,
    },
    /// `^` 输入起始锚点
    Start,
    /// `$` 输入结束锚点
    End,
    /// 分组（`index` 为捕获组编号，从 1 起；`None` 为非捕获组）
    Group {
        /// 捕获组编号
        index: Option<usize>,
        /// 组内子模式
        node: Box<Node>,
    },
    /// 后行断言 `(?<=...)` / `(?<!...)`：要求子模式匹配恰好终止于当前位置
    Lookbehind {
        /// `true` 为负向后行 `(?<!...)`
        negated: bool,
        /// 断言子模式
        node: Box<Node>,
    },
    /// 先行断言 `(?=...)` / `(?!...)`：子模式从当前位置匹配即成功（零宽）
    Lookahead {
        /// `true` 为负向先行 `(?!...)`
        negated: bool,
        /// 断言子模式
        node: Box<Node>,
    },
    /// 捕获组反向引用 `\k<name>` 与 `\1..`\9`
    Backref(usize),
    /// 词边界断言 `\b` / `\B`（词字符 = 字母数字或 `_`）
    WordBoundary {
        /// `true` 为非词边界 `\B`
        negated: bool,
    },
    /// 顺序连接
    Concat(Vec<Node>),
    /// 选择分支（按序尝试）
    Alt(Vec<Node>),
    /// 量词重复
    Repeat {
        /// 被重复的子模式
        node: Box<Node>,
        /// 最少次数
        min: u32,
        /// 最多次数（`None` 表示无上界）
        max: Option<u32>,
        /// 是否贪婪
        greedy: bool,
    },
}

/// 解析产出的正则程序。
pub(crate) struct Parsed {
    /// 根节点
    pub(crate) root: Node,
    /// 捕获组总数
    pub(crate) group_count: usize,
    /// 捕获组命名（下标 gi-1 对应组号 gi；未命名的组为 `None`）
    pub(crate) group_names: Vec<Option<String>>,
}

/// 递归下降解析器。
struct Parser {
    chars: Vec<char>,
    pos: usize,
    group_count: usize,
    /// 捕获组命名表（与组号同步追加）
    group_names: Vec<Option<String>>,
    /// `try_parse_counted` 解析出的 `{m,n}` 暂存槽
    last_counted: Option<(u32, Option<u32>)>,
}

impl Parser {
    fn parse(mut self) -> Result<Parsed, RegexError> {
        let root = self.parse_alt()?;
        if self.pos != self.chars.len() {
            return Err(self.err("unexpected trailing input"));
        }
        Ok(Parsed {
            root,
            group_count: self.group_count,
            group_names: self.group_names,
        })
    }

    fn err(&self, msg: &str) -> RegexError {
        RegexError::Syntax(format!("{msg} at position {}", self.pos))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `alt := concat ('|' concat)*`
    fn parse_alt(&mut self) -> Result<Node, RegexError> {
        let mut branches = vec![self.parse_concat()?];
        while self.eat('|') {
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("len>=1")
        } else {
            Node::Alt(branches)
        })
    }

    /// `concat := repeat*`（`|` 与 `)` 终止）
    fn parse_concat(&mut self) -> Result<Node, RegexError> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            items.push(self.parse_repeat()?);
        }
        Ok(match items.len() {
            0 => Node::Concat(Vec::new()),
            1 => items.pop().expect("len==1"),
            _ => Node::Concat(items),
        })
    }

    /// `repeat := atom quantifier? lazy?`
    fn parse_repeat(&mut self) -> Result<Node, RegexError> {
        let atom = self.parse_atom()?;
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') if self.try_parse_counted() => {
                // try_parse_counted 已消费 `{m,n}` 并暂存结果
                self.last_counted.take().expect("counted 量词已解析")
            }
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    /// 尝试解析 `{m}` / `{m,}` / `{m,n}`；成功则消费并把结果存入 `last_counted`。
    fn try_parse_counted(&mut self) -> bool {
        let save = self.pos;
        self.pos += 1; // 吃掉 '{'
        let Some(min) = self.parse_number() else {
            self.pos = save;
            return false;
        };
        let max = if self.eat(',') {
            self.parse_number()
        } else {
            Some(min)
        };
        if !self.eat('}') {
            self.pos = save;
            return false;
        }
        self.last_counted = Some((min, max));
        true
    }

    fn parse_number(&mut self) -> Option<u32> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if start == self.pos {
            return None;
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse()
            .ok()
    }

    /// `atom := group | class | anchor | escape | '.' | literal`
    fn parse_atom(&mut self) -> Result<Node, RegexError> {
        let c = self.bump().ok_or_else(|| self.err("unexpected end"))?;
        match c {
            '(' => self.parse_group(),
            '[' => Ok(Node::Class {
                negated: self.eat('^'),
                items: self.parse_class_items()?,
            }),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '\\' => self.parse_escape_node(),
            '*' | '+' | '?' => Err(self.err("dangling quantifier")),
            other => Ok(Node::Char(other)),
        }
    }

    /// `group := '(' '?:'? alt ')'`（捕获组编号按 `(` 出现顺序）。
    ///
    /// 支持命名捕获组 `(?<name>...)` 与后行断言 `(?<=...)` / `(?<!...)`。
    fn parse_group(&mut self) -> Result<Node, RegexError> {
        let index = if self.peek() == Some('?') {
            self.pos += 1;
            match self.peek() {
                Some(':') => {
                    self.pos += 1;
                    None
                }
                // 先行断言 `(?=...)` / `(?!...)`（零宽；捕获组号不受影响）
                Some('=') | Some('!') => {
                    let negated = self.peek() == Some('!');
                    self.pos += 1;
                    let node = self.parse_alt()?;
                    if !self.eat(')') {
                        return Err(self.err("unbalanced parenthesis"));
                    }
                    return Ok(Node::Lookahead {
                        negated,
                        node: Box::new(node),
                    });
                }
                // 命名捕获组 `(?<name>...)`：命名组同样占用组号
                Some('<') if self.peek_ahead_is(Some('=')) || self.peek_ahead_is(Some('!')) => {
                    // 后行断言 `(?<=...)` / `(?<!...)`
                    let negated = self.peek_ahead_is(Some('!'));
                    self.pos += 2; // 吃掉 '<' 与 '='/'!'
                    let node = self.parse_alt()?;
                    if !self.eat(')') {
                        return Err(self.err("unbalanced parenthesis"));
                    }
                    return Ok(Node::Lookbehind {
                        negated,
                        node: Box::new(node),
                    });
                }
                Some('<') => {
                    self.pos += 1; // 吃掉 '<'
                    let mut name = String::new();
                    loop {
                        match self.bump() {
                            Some('>') => break,
                            Some(c) => name.push(c),
                            None => return Err(self.err("unterminated group name")),
                        }
                    }
                    if name.is_empty() {
                        return Err(self.err("empty group name"));
                    }
                    self.group_count += 1;
                    self.group_names.push(Some(name));
                    Some(self.group_count)
                }
                Some(other) => {
                    return Err(self.err(&format!("unsupported group modifier '{other}'")));
                }
                None => return Err(self.err("unterminated group")),
            }
        } else {
            self.group_count += 1;
            self.group_names.push(None);
            Some(self.group_count)
        };
        let node = self.parse_alt()?;
        if !self.eat(')') {
            return Err(self.err("unbalanced parenthesis"));
        }
        Ok(Node::Group {
            index,
            node: Box::new(node),
        })
    }

    /// 判断当前位置后第 2 个字符是否为 `c`（调用点已确认 `pos` 处为 `<`）。
    fn peek_ahead_is(&self, c: Option<char>) -> bool {
        self.chars.get(self.pos + 1).copied() == c
    }

    /// 字符类成员直到 `]`（调用方已消费 `[` 与可选 `^`）。
    ///
    /// 成员以码点（u32）参与范围判定——`\xHH` / `\uHHHH` 转义可产生代理区
    /// 码点（如 lodash 的 `[\ud800-\udfff]`），Rust `char` 无法承载。
    fn parse_class_items(&mut self) -> Result<Vec<ClassItem>, RegexError> {
        let mut items = Vec::new();
        // `]` 作为首字符是字面量
        if self.peek() == Some(']') {
            self.pos += 1;
            items.push(ClassItem::Ch(']'));
        }
        loop {
            let c = self.bump().ok_or_else(|| self.err("unterminated class"))?;
            if c == ']' {
                return Ok(items);
            }
            let lo = if c == '\\' {
                match self.parse_class_escape()? {
                    ClassEscape::Char(ch) => ch as u32,
                    ClassEscape::Cp(cp) => cp,
                    ClassEscape::Shorthand(item) => {
                        items.push(item);
                        continue;
                    }
                }
            } else {
                c as u32
            };
            // 范围 `a-z`（`-` 在 `]` 前是字面量）
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1;
                let hi_raw = self.bump().expect("peek 已确认存在");
                let hi = if hi_raw == '\\' {
                    match self.parse_class_escape()? {
                        ClassEscape::Char(ch) => ch as u32,
                        ClassEscape::Cp(cp) => cp,
                        ClassEscape::Shorthand(_) => {
                            return Err(self.err("shorthand as range bound"));
                        }
                    }
                } else {
                    hi_raw as u32
                };
                if hi < lo {
                    return Err(self.err("range out of order"));
                }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(Self::cp_item(lo));
            }
        }
    }

    /// 码点 → 类成员（代理区码点无法转 `char`，以 [`ClassItem::Cp`] 承载）。
    fn cp_item(cp: u32) -> ClassItem {
        match char::from_u32(cp) {
            Some(c) => ClassItem::Ch(c),
            None => ClassItem::Cp(cp),
        }
    }

    /// 类内转义：`\d \D \w \W \s \S` 或字面字符（含 `\xHH` / `\uHHHH` /
    /// `\u{...}` 码点转义与 `\0` NUL——此前缺失致 `[^\x00-\x2f...]` 类否定
    /// 集完全错乱，lodash `words` 的 reAsciiWord 实测匹配出 `[" ","-","z"]`）。
    fn parse_class_escape(&mut self) -> Result<ClassEscape, RegexError> {
        let c = self.bump().ok_or_else(|| self.err("unterminated escape"))?;
        Ok(match c {
            'd' => ClassEscape::Shorthand(ClassItem::Digit),
            'D' => ClassEscape::Shorthand(ClassItem::NotDigit),
            'w' => ClassEscape::Shorthand(ClassItem::Word),
            'W' => ClassEscape::Shorthand(ClassItem::NotWord),
            's' => ClassEscape::Shorthand(ClassItem::Space),
            'S' => ClassEscape::Shorthand(ClassItem::NotSpace),
            'n' => ClassEscape::Char('\n'),
            't' => ClassEscape::Char('\t'),
            'r' => ClassEscape::Char('\r'),
            'f' => ClassEscape::Char('\u{c}'),
            'v' => ClassEscape::Char('\u{b}'),
            // `\0`：NUL（后随数字的 `\08` 形态语料未涉，按 NUL 处理）
            '0' => ClassEscape::Char('\0'),
            'x' => ClassEscape::Char(self.parse_hex_escape(2)?),
            'u' => {
                let cp = self.parse_unicode_escape()?;
                match char::from_u32(cp) {
                    Some(c) => ClassEscape::Char(c),
                    None => ClassEscape::Cp(cp),
                }
            }
            'b' => ClassEscape::Char('\u{8}'),
            other => ClassEscape::Char(other),
        })
    }

    /// 读取恰好 `n` 位十六进制数字（`\xHH` / `\uHHHH` 的定宽形式）。
    fn parse_hex_escape(&mut self, n: usize) -> Result<char, RegexError> {
        let mut v = 0u32;
        for _ in 0..n {
            let c = self
                .bump()
                .ok_or_else(|| self.err("unterminated hex escape"))?;
            let d = c.to_digit(16).ok_or_else(|| self.err("bad hex escape"))?;
            v = v * 16 + d;
        }
        char::from_u32(v).ok_or_else(|| self.err("hex escape out of range"))
    }

    /// `\uHHHH` 定宽或 `\u{H+}` 括号码点转义（返回原始码点，代理区不折叠）。
    fn parse_unicode_escape(&mut self) -> Result<u32, RegexError> {
        if self.peek() == Some('{') {
            self.pos += 1;
            let mut v = 0u32;
            loop {
                let c = self
                    .bump()
                    .ok_or_else(|| self.err("unterminated \\u{ escape"))?;
                if c == '}' {
                    break;
                }
                let d = c
                    .to_digit(16)
                    .ok_or_else(|| self.err("bad \\u{ code point"))?;
                v = v * 16 + d;
                if v > 0x10_FFFF {
                    return Err(self.err("code point out of range"));
                }
            }
            return Ok(v);
        }
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self
                .bump()
                .ok_or_else(|| self.err("unterminated unicode escape"))?;
            let d = c
                .to_digit(16)
                .ok_or_else(|| self.err("bad unicode escape"))?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    /// 类外转义：`\d \D \w \W \s \S` 展开为单成员字符类；`\k<name>` 命名组
    /// 反向引用与 `\1..\9` 数字反向引用；其余为字面字符
    /// （`\b`/`\B` 词边界语料外，显式报语法错误）。
    fn parse_escape_node(&mut self) -> Result<Node, RegexError> {
        use ClassItem::{Digit, NotSpace, NotWord, Space, Word};
        let c = self.bump().ok_or_else(|| self.err("unterminated escape"))?;
        Ok(match c {
            'n' => Node::Char('\n'),
            't' => Node::Char('\t'),
            'r' => Node::Char('\r'),
            'f' => Node::Char('\u{c}'),
            'v' => Node::Char('\u{b}'),
            // `\0`：NUL（后随数字时按规范不做此解释，语料未涉）
            '0' if !self.peek().is_some_and(|ch| ch.is_ascii_digit()) => Node::Char('\0'),
            // `\xHH` / `\uHHHH` / `\u{H+}`：码点转义（代理区码点包成单成员类）
            'x' => Node::Char(self.parse_hex_escape(2)?),
            'u' => {
                let cp = self.parse_unicode_escape()?;
                match char::from_u32(cp) {
                    Some(c) => Node::Char(c),
                    None => Node::Class {
                        negated: false,
                        items: vec![ClassItem::Cp(cp)],
                    },
                }
            }
            'd' => Node::Class {
                negated: false,
                items: vec![Digit],
            },
            'D' => Node::Class {
                negated: true,
                items: vec![Digit],
            },
            'w' => Node::Class {
                negated: false,
                items: vec![Word],
            },
            'W' => Node::Class {
                negated: true,
                items: vec![NotWord],
            },
            's' => Node::Class {
                negated: false,
                items: vec![Space],
            },
            'S' => Node::Class {
                negated: true,
                items: vec![NotSpace],
            },
            // `\k<name>`：命名组反向引用（组须已定义）
            'k' if self.peek() == Some('<') => {
                self.pos += 1; // 吃掉 '<'
                let mut name = String::new();
                loop {
                    match self.bump() {
                        Some('>') => break,
                        Some(ch) => name.push(ch),
                        None => return Err(self.err("unterminated group name")),
                    }
                }
                let gi = self
                    .group_names
                    .iter()
                    .position(|n| n.as_deref() == Some(name.as_str()))
                    .map(|i| i + 1)
                    .ok_or_else(|| self.err(&format!("group name '{name}' not defined")))?;
                Node::Backref(gi)
            }
            // `\1..`\9`：数字反向引用（组须已存在；多位数字一并解析）
            '1'..='9' => {
                let mut num = c.to_digit(10).ok_or_else(|| self.err("bad backref"))? as usize;
                while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                    num = num * 10
                        + (self
                            .bump()
                            .expect("peek 已确认")
                            .to_digit(10)
                            .expect("digit") as usize);
                }
                if num > self.group_names.len() {
                    return Err(self.err(&format!("backreference to nonexistent group {num}")));
                }
                Node::Backref(num)
            }
            // `\b` / `\B`：词边界断言（类内 `\b` 由 parse_class_escape 处理为字面量）
            'b' => Node::WordBoundary { negated: false },
            'B' => Node::WordBoundary { negated: true },
            other => Node::Char(other),
        })
    }
}

/// 类内转义结果。
enum ClassEscape {
    /// 字面字符
    Char(char),
    /// 代理区码点（`\uD800..=\uDFFF`——`char` 无法表示）
    Cp(u32),
    /// 类简写成员
    Shorthand(ClassItem),
}

/// 解析模式字符串。
pub(crate) fn parse(pattern: &str) -> Result<Parsed, RegexError> {
    let p = Parser {
        chars: pattern.chars().collect(),
        pos: 0,
        group_count: 0,
        group_names: Vec::new(),
        last_counted: None,
    };
    p.parse()
}
