//! 递归下降语法分析器（JS / TS 源码 → AST）。
//!
//! 支持 ECMAScript 核心文法、Class、Try/Catch、可选链，以及 TypeScript 类型注解零成本剥离。

use crate::ast::{
    ArrayPatternElem, ClassMethodDef, ExportDecl, ExportSpecifier, Expr, FunctionDef, ImportDecl,
    ImportSpecifier, ObjectPatternProp, ObjectProp, Program, PropKey, PropValue, SpannedStmt, Stmt,
    SwitchCase, VarKind, VarPattern,
};
use crate::lexer::{Lexer, Token, TokenKind};

/// 类体解析产物：`(super, constructor, methods, fields)`——`class` 关键字
/// 之后的共享尾部（语句与表达式两种形态共用）。
type ClassTail = (
    Option<Box<Expr>>,
    Option<FunctionDef>,
    Vec<ClassMethodDef>,
    Vec<(String, bool, Option<Expr>)>,
);

/// 语法分析器。
pub struct Parser<'src> {
    tokens: Vec<Token>,
    pos: usize,
    src: &'src str,
    /// 行号游标：已统计到 `line_pos` 字节处，对应 `line_no` 行（LCOV 行覆盖）。
    line_pos: usize,
    line_no: u32,
    /// 语法错误收集。`parse` 维持容错（错误不阻断、AST 尽力而为，兼容
    /// 既有调用方）；`parse_strict`/`take_errors` 供 alukac 等需要拒绝
    /// 非法源码的入口使用。
    errors: Vec<String>,
    /// 当前是否处于 async 函数体内（await 早错误判定：
    /// async 上下文中 await 为保留字，不得作标识符/标签）
    in_async: bool,
    /// 普通函数（声明/表达式）体内 super 不可用（无 HomeObject）——
    /// super()/super.x 均为 SyntaxError；类体/对象字面量方法内恢复合法
    super_disallowed: bool,
    /// 程序级 strict 语义（首 token 为 "use strict" 指令时置位）——
    /// 简单形参名重复（StrictFormalParameters）等 strict 早错误判定用
    strict: bool,
    /// 是否处于生成器函数体内（yield 为生成器运算符；非生成器语境
    /// `yield` 是普通标识符——`var yield = 'y'` / `get [yield]()`）
    in_generator: bool,
    /// TypeScript 解析语境（`.ts/.mts/.cts/.tsx`）。仅在**语法歧义**处影响
    /// 判定（`f<T>(x)` 的泛型实参 vs `a < b > (c)` 比较链、`x!` 的非空断言
    /// vs 换行后 `!y` 的 ASI）；无歧义的 TS 形态（类型注解、成员修饰符）在
    /// 两种语境下都按 TS 解释——它们在 JS 里本就是语法错误。
    ts: bool,
}

/// 解析源码文本为 AST 语法树。
#[must_use]
pub fn parse(src: &str) -> Program {
    let mut parser = Parser::new(src);
    parser.parse_program()
}

/// 严格解析：源码含语法错误时返回 `Err`（消息含全部错误）。
pub fn parse_strict(src: &str) -> Result<Program, String> {
    let mut parser = Parser::new(src);
    let program = parser.parse_program();
    if parser.errors.is_empty() {
        Ok(program)
    } else {
        Err(parser.errors.join("; "))
    }
}

/// For 循环分类枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForLoopKind {
    ForIn,
    ForOf,
    Standard,
}

impl<'src> Parser<'src> {
    /// 创建语法解析器实例。
    #[must_use]
    pub fn new(src: &'src str) -> Self {
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token();
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Self {
            tokens,
            pos: 0,
            src,
            line_pos: 0,
            line_no: 1,
            errors: Vec::new(),
            in_async: false,
            strict: false,
            in_generator: false,
            super_disallowed: false,
            ts: false,
        }
    }

    /// 置位 TypeScript 解析语境（见 [`Parser`] 的 `ts` 字段）。
    pub fn set_typescript_mode(&mut self, on: bool) {
        self.ts = on;
    }

    /// 当前 token 的源码行号（自游标增量统计换行，均摊 O(n)）。
    fn cur_line(&mut self) -> u32 {
        let start = self.peek().start;
        if start >= self.line_pos {
            let nl = self.src[self.line_pos..start]
                .bytes()
                .filter(|b| *b == 0x0A)
                .count() as u32;
            self.line_no += nl;
            self.line_pos = start;
        }
        self.line_no
    }

    /// 以语句起始行号包装为 [`SpannedStmt`]。
    fn at(line: u32, stmt: Stmt) -> SpannedStmt {
        SpannedStmt::new(stmt, line)
    }

    /// 记录一条语法错误（容错解析继续，strict 入口据此拒绝）。
    fn record_error(&mut self, message: String) {
        self.errors.push(message);
    }

    /// 取出已收集的语法错误（解析完成后调用一次）。
    #[must_use]
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            self.tokens.last().unwrap()
        }
    }

    fn peek_ahead(&self, n: usize) -> &Token {
        let idx = self.pos + n;
        if idx < self.tokens.len() {
            &self.tokens[idx]
        } else {
            self.tokens.last().unwrap()
        }
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check_punct(&self, p: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Punct(s) if s == p)
    }

    fn match_punct(&mut self, p: &str) -> bool {
        if self.check_punct(p) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check_keyword(&self, kw: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Keyword(s) if s == kw)
    }

    /// 上下文关键字作**标识符**：`async`/`await`/`yield`/`from`/`as`/
    /// `static`/`get`/`set`/`of` 在规范中均为普通标识符（仅在特定产生式
    /// 位置才具关键字语义）。真实包大量使用（axios 的 `async` 变量、
    /// asynckit 的 `async` 形参/属性简写）。
    fn context_ident(&self) -> Option<String> {
        match &self.peek().kind {
            TokenKind::Keyword(k) => {
                // `await` 在 **async 语境** 是保留字（不得作绑定名：
                // `async function f(){ var await = 1 }` → SyntaxError，
                // S7.6.1 负例族）；非 async 语境才是普通标识符
                if k == "await" && self.in_async {
                    return None;
                }
                if matches!(
                    k.as_str(),
                    "async" | "await" | "yield" | "from" | "as" | "static" | "get" | "set" | "of"
                ) {
                    Some(k.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 消耗一个「标识符或上下文关键字」并返回其名。
    fn advance_ident_like(&mut self) -> Option<String> {
        if let TokenKind::Ident(id) = self.peek().kind.clone() {
            self.advance();
            return Some(id);
        }
        if let Some(k) = self.context_ident() {
            self.advance();
            return Some(k);
        }
        None
    }

    fn match_keyword(&mut self, kw: &str) -> bool {
        if self.check_keyword(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), String> {
        if self.match_punct(p) {
            Ok(())
        } else {
            let message = format!("预期标点 '{}', 实为 '{:?}'", p, self.peek());
            self.record_error(message.clone());
            Err(message)
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        if self.match_keyword(kw) {
            Ok(())
        } else {
            let message = format!("预期关键字 '{}', 实为 '{:?}'", kw, self.peek());
            self.record_error(message.clone());
            Err(message)
        }
    }

    fn scan_for_loop_kind(&self) -> ForLoopKind {
        let mut depth = 0;
        let mut idx = self.pos;
        while idx < self.tokens.len() {
            let tok = &self.tokens[idx];
            match &tok.kind {
                TokenKind::Punct(p) if p == "(" || p == "[" || p == "{" => {
                    depth += 1;
                }
                TokenKind::Punct(p) if p == ")" || p == "]" || p == "}" => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Punct(p) if p == ";" && depth == 0 => {
                    return ForLoopKind::Standard;
                }
                TokenKind::Keyword(k) if k == "in" && depth == 0 => {
                    return ForLoopKind::ForIn;
                }
                TokenKind::Keyword(k) if k == "of" && depth == 0 => {
                    return ForLoopKind::ForOf;
                }
                TokenKind::Ident(id) if id == "of" && depth == 0 => {
                    return ForLoopKind::ForOf;
                }
                _ => {}
            }
            idx += 1;
        }
        ForLoopKind::Standard
    }

    /// 跳过可选的分号
    /// 下一 token 之前是否出现行终止符（ASI 判定用：扫描上一 token 结束
    /// 到当前 token 起点之间的源码间隙）。
    fn nl_before_current(&self) -> bool {
        if self.pos == 0 || self.pos >= self.tokens.len() {
            return false;
        }
        let prev = &self.tokens[self.pos - 1];
        let cur = &self.tokens[self.pos];
        let prev_end = prev.start + prev.text.len();
        if prev_end > cur.start {
            return false; // LexError 等跨界载体，保守按无换行
        }
        self.src[prev_end..cur.start].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    }

    /// 语句收尾分号：**宽松吞掉可选 `;`**。
    ///
    /// 严格 ASI（限换行/`}`/EOF 三情形）在 20260912 轮三实测净负：
    /// 误伤 > 修复（hashbang 未剥离、`/` 除法-正则歧义、空文本 token 等
    /// 前置缺陷被暴露为误报，t262 基线 657→642）。回退宽松态；严格化
    /// 需先补齐 hashbang 剥离与语句模型清理后再启用（nl_before_current
    /// 助手保留备用）。
    /// 指令序言形态判定：`Stmt::Expr(字符串字面量)`。
    fn is_directive(s: &SpannedStmt) -> bool {
        matches!(s.stmt, Stmt::Expr(Expr::String(_)))
    }

    /// 语句收尾分号：显式 `;`，或 ASI 三情形（下一 token 前有换行 /
    /// `}` / EOF）自动补。其余形态记录错误——轮十三重启严格化：
    /// hashbang/U+2028 前置已解除，实测净效应见 t262 基线。
    fn eat_semi(&mut self) {
        if self.match_punct(";") {
            return;
        }
        let asi_ok =
            self.peek().kind == TokenKind::Eof || self.check_punct("}") || self.nl_before_current();
        if !asi_ok {
            let text = self.peek().text.clone();
            self.record_error(format!(
                "SyntaxError: 预期 ';'，同行遇到 '{text}'（自动分号插入不适用）"
            ));
        }
    }

    /// 跳过 TypeScript 类型注解（例如 `: number`, `: Array<string>`,
    /// `: (x: number) => void`, `: { a: number }` 等）。
    fn skip_type_annotation(&mut self) {
        if self.match_punct(":") {
            self.skip_type();
        }
    }

    /// 跳过一段 TypeScript 类型。
    ///
    /// 状态机：`need_primary`（期待类型原语）↔ 已取得原语（可接后缀）。
    /// 只有**确定属于类型**的 token 才被消耗，因此既覆盖
    /// `readonly T[]` / `keyof T` / `Array<Map<string, T>>` / `{ a: number }` /
    /// `(x: number) => void` / `A | B`，也不会把后续语句的首个标识符吞进
    /// 类型里（`const a = b as Foo` 换行 `const c = 1` 的 ASI 语义）。
    fn skip_type(&mut self) {
        let mut need_primary = true;
        // 上一个原语是否为括号组：只有函数类型形参表（`(a: X) => Y`）之后的
        // `=>` 才属于类型本身；`(x: string): string => x` 里的 `=>` 是**箭头
        // 函数**的语法记号，必须留给调用方，否则返回值注解会把箭头吃掉
        let mut last_was_paren = false;
        while self.peek().kind != TokenKind::Eof {
            let kind = self.peek().kind.clone();
            let text = self.peek().text.clone();
            match &kind {
                // 类型前缀关键字：其后仍须一个原语（`readonly number[]`）
                TokenKind::Ident(_) | TokenKind::Keyword(_)
                    if need_primary
                        && matches!(
                            text.as_str(),
                            "readonly"
                                | "keyof"
                                | "typeof"
                                | "infer"
                                | "unique"
                                | "new"
                                | "abstract"
                                | "asserts"
                        ) =>
                {
                    self.advance();
                }
                // 类型原语：限定名首段 / 字面量类型 / 泛型名
                TokenKind::Ident(_)
                | TokenKind::Keyword(_)
                | TokenKind::String(_)
                | TokenKind::Number(_)
                | TokenKind::BigInt(_)
                    if need_primary =>
                {
                    self.advance();
                    need_primary = false;
                    last_was_paren = false;
                }
                // 负数字面量类型（`-1`）
                TokenKind::Punct(p) if p == "-" && need_primary => {
                    self.advance();
                }
                // 复合原语：括号类型 / 对象类型 / 元组类型 / 函数类型形参表
                TokenKind::Punct(p) if p == "(" && need_primary => {
                    self.skip_balanced("(", ")");
                    need_primary = false;
                    last_was_paren = true;
                }
                TokenKind::Punct(p) if p == "{" && need_primary => {
                    self.skip_balanced("{", "}");
                    need_primary = false;
                    last_was_paren = false;
                }
                TokenKind::Punct(p) if p == "[" && need_primary => {
                    self.skip_balanced("[", "]");
                    need_primary = false;
                    last_was_paren = false;
                }
                // 后缀：泛型实参 / 数组与下标 / 限定名
                TokenKind::Punct(p) if p == "<" && !need_primary => {
                    if !self.skip_angle_group() {
                        break;
                    }
                }
                TokenKind::Punct(p) if p == "[" && !need_primary => {
                    self.skip_balanced("[", "]");
                }
                TokenKind::Punct(p) if p == "." && !need_primary => {
                    self.advance();
                    need_primary = true;
                }
                // 类型谓词：`value is Plugin` / `err is Error`（`is` 在词法层
                // 是普通标识符，只在已取得原语后的位置作类型连接词）
                TokenKind::Ident(w) if w == "is" && !need_primary => {
                    self.advance();
                    need_primary = true;
                }
                // 组合子：联合 / 交叉 / 函数类型箭头 / 元组展开
                TokenKind::Punct(p) if p == "|" || p == "&" => {
                    self.advance();
                    need_primary = true;
                }
                TokenKind::Punct(p) if p == "=>" && !need_primary && last_was_paren => {
                    self.advance();
                    need_primary = true;
                    last_was_paren = false;
                }
                TokenKind::Punct(p) if p == "..." && need_primary => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    /// 平衡扫描一对定界符（含嵌套）。未配对时停在 EOF。
    fn skip_balanced(&mut self, open: &str, close: &str) -> bool {
        if !self.check_punct(open) {
            return false;
        }
        let mut depth = 0usize;
        while self.peek().kind != TokenKind::Eof {
            if let TokenKind::Punct(p) = &self.peek().kind {
                if p == open {
                    depth += 1;
                } else if p == close {
                    depth -= 1;
                    if depth == 0 {
                        self.advance();
                        return true;
                    }
                }
            }
            self.advance();
        }
        false
    }

    /// 平衡扫描 `<...>`：`>>`/`>>>` 是单 token，按字符数一次收口多层。
    ///
    /// 类型实参内部可以出现括号/方括号/花括号（`Promise<{ a: number; b: string }>`、
    /// `Map<string, Array<T>>`），故 `;` 只在**括号嵌套归零**时才是语句边界；
    /// 嵌套内的 `;` 属对象类型成员分隔符。
    fn skip_angle_group(&mut self) -> bool {
        if !self.check_punct("<") {
            return false;
        }
        let mut depth = 0usize;
        let mut inner = 0i32;
        while self.peek().kind != TokenKind::Eof {
            if let TokenKind::Punct(p) = &self.peek().kind {
                match p.as_str() {
                    "(" | "[" | "{" => inner += 1,
                    ")" | "]" | "}" if inner > 0 => inner -= 1,
                    "<" => depth += 1,
                    ";" if inner == 0 => {
                        // 语句边界：不可能是类型实参内容 —— 按比较链解释
                        return false;
                    }
                    _ if p.starts_with('>') && p.chars().all(|c| c == '>') => {
                        if p.len() >= depth {
                            depth = 0;
                        } else {
                            depth -= p.len();
                        }
                        if depth == 0 {
                            self.advance();
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            self.advance();
        }
        false
    }

    /// 声明或调用位置的可选类型参数列表 `<T, U extends X = Y>`。
    ///
    /// 判定为「类型参数」而非「小于号」的条件：尖括号平衡成立，且**后继
    /// token** 属于 {`(`, `{`, `=>`, `extends`, `implements`}（逗号、换行等
    /// 位置一律按比较链解释）。失败时游标回退并返回 false。
    fn try_skip_type_args(&mut self) -> bool {
        if !self.check_punct("<") {
            return false;
        }
        let save = self.pos;
        if !self.skip_angle_group() {
            self.pos = save;
            return false;
        }
        let next_ok = self.check_punct("(")
            || self.check_punct("{")
            || self.check_punct("=>")
            || self.check_keyword("extends")
            || self.check_soft_keyword("implements");
        if !next_ok {
            self.pos = save;
            return false;
        }
        true
    }

    /// 剥离语句位的 TS 类型层声明：`interface` / `type` / `declare`。
    ///
    /// 三者都是**软关键字**（可作普通标识符），故仅在形态明确时才吞：
    /// `interface X` 后必须随标识符、`type X` 后必须随 `=`/`<`、`declare`
    /// 后必须随一个声明关键字（`declare = 1` 仍是赋值语句）。
    fn skip_ts_declaration(&mut self) -> bool {
        if self.check_soft_keyword("interface")
            && matches!(self.peek_ahead(1).kind, TokenKind::Ident(_))
        {
            self.advance();
            self.advance();
            let _ = self.try_skip_type_args();
            if self.match_keyword("extends") {
                loop {
                    self.skip_type();
                    if !self.match_punct(",") {
                        break;
                    }
                }
            }
            if self.check_punct("{") {
                self.skip_balanced("{", "}");
            }
            return true;
        }
        if self.check_soft_keyword("type")
            && matches!(self.peek_ahead(1).kind, TokenKind::Ident(_))
            && (self.peek_ahead(2).is_punct("=") || self.peek_ahead(2).is_punct("<"))
        {
            while !self.check_punct(";") && self.peek().kind != TokenKind::Eof {
                self.advance();
            }
            self.eat_semi();
            return true;
        }
        if self.check_soft_keyword("declare")
            && matches!(
                &self.peek_ahead(1).kind,
                TokenKind::Ident(w) | TokenKind::Keyword(w)
                    if matches!(
                        w.as_str(),
                        "class"
                            | "const"
                            | "function"
                            | "enum"
                            | "interface"
                            | "let"
                            | "module"
                            | "namespace"
                            | "var"
                            | "global"
                            | "type"
                            | "abstract"
                    )
            )
        {
            self.advance();
            // 环境声明体（`declare namespace N { ... }` / `declare global { ... }`）
            loop {
                match &self.peek().kind {
                    TokenKind::Eof => break,
                    TokenKind::Punct(p) if p == "{" => {
                        let _ = self.skip_balanced("{", "}");
                        break;
                    }
                    TokenKind::Punct(p) if p == ";" => {
                        self.eat_semi();
                        break;
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
            return true;
        }
        false
    }

    /// 软关键字匹配（TS 关键字在词法层有的是标识符、有的入关键字表，
    /// 两种词法形态都接受）。
    fn match_soft_keyword(&mut self, word: &str) -> bool {
        if self.check_soft_keyword(word) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 软关键字判定（不消耗）。
    fn check_soft_keyword(&self, word: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w) | TokenKind::Keyword(w) if w == word)
    }

    /// `implements A, B<C>` 子句（存在则跳过）。
    fn skip_implements_clause(&mut self) {
        if self.match_soft_keyword("implements") {
            loop {
                self.skip_type();
                if !self.match_punct(",") {
                    break;
                }
            }
        }
    }

    /// 形参名后的 TS 后缀：`?` 可选标记 → `!` → `: 类型`。
    fn skip_param_suffix(&mut self) {
        let _ = self.match_punct("?");
        let _ = self.match_punct("!");
        self.skip_type_annotation();
    }

    /// `this: T` 伪形参（TS 语境；剥离后不占实参位）。
    fn check_this_param(&self) -> bool {
        self.check_keyword("this") && self.peek_ahead(1).is_punct(":")
    }

    /// 类成员前导修饰符（TS）：返回 `(是否 static, 是否 abstract/declare)`。
    ///
    /// 全部是软关键字——仅当**后随另一修饰符或成员键**（标识符 / `#` /
    /// `[` / 字符串 / 数字）时才按修饰符消耗；否则该单词就是成员名本身
    /// （`private = 1`、`static() {}`、`readonly;` 皆属此类）。
    fn take_member_modifiers(&mut self) -> (bool, bool) {
        let mut is_static = false;
        let mut is_erased = false;
        while let TokenKind::Ident(word) | TokenKind::Keyword(word) = self.peek().kind.clone() {
            let kind = match word.as_str() {
                "public" | "private" | "protected" => 1,
                "readonly" => 2,
                "override" => 3,
                "static" => 4,
                "abstract" => 5,
                "declare" => 6,
                _ => break,
            };
            // 后随须为成员键或另一修饰符：`private x` / `private [k]` /
            // `static readonly x`；`private` 后随 `(`/`=`/`;`/`:`/`?` 时
            // 是成员名而非修饰符
            let follows = match &self.peek_ahead(1).kind {
                TokenKind::Ident(w) | TokenKind::Keyword(w) => {
                    matches!(
                        w.as_str(),
                        "public"
                            | "private"
                            | "protected"
                            | "readonly"
                            | "override"
                            | "static"
                            | "abstract"
                            | "declare"
                            | "async"
                            | "get"
                            | "set"
                    ) || !w.is_empty()
                }
                TokenKind::String(_) | TokenKind::Number(_) | TokenKind::Punct(_) => {
                    matches!(&self.peek_ahead(1).kind, TokenKind::Punct(p) if p == "#" || p == "[" || p == "*")
                }
                _ => false,
            };
            if !follows {
                break;
            }
            self.advance();
            match kind {
                4 => is_static = true,
                5 | 6 => is_erased = true,
                _ => {}
            }
        }
        (is_static, is_erased)
    }

    /// 擦除 `abstract` / `declare` 成员的声明（strip-only 语义：类型层面
    /// 的成员不产生任何运行时可见对象——Node 22 实测 `abstract m(): T;`
    /// 剥离后原型上无该方法）。
    fn skip_erased_member(&mut self) {
        if self.check_punct("*") {
            self.advance();
        }
        if self.check_punct("[") {
            self.skip_balanced("[", "]");
        } else if self.peek().kind != TokenKind::Eof {
            self.advance();
        }
        if self.check_punct("(") {
            self.skip_balanced("(", ")");
        }
        let _ = self.match_punct("?");
        let _ = self.match_punct("!");
        self.skip_type_annotation();
        if self.check_punct("{") {
            self.skip_balanced("{", "}");
        } else {
            self.eat_semi();
        }
    }

    /// 解析完整 Program
    /// ESM 顶层隐式 async 语境（顶层 await / TLA 合法）。
    pub fn set_esm_top_level_async(&mut self) {
        self.in_async = true;
    }

    /// 解析完整 Program（脚本/模块的顶层语句列表）。
    pub fn parse_program(&mut self) -> Program {
        // 程序级 strict 指令：首个 token 为 "use strict" 字面量时全程序
        // 按 strict 语义解析（onlyStrict 变体 / 顶层指令）
        if matches!(
            self.tokens.first().map(|t| &t.kind),
            Some(TokenKind::String(s)) if s == "use strict"
        ) {
            self.strict = true;
        }
        let mut body = Vec::new();
        loop {
            // 词法错误（未终止多行注释等）：任意位置判死
            if let TokenKind::LexError(msg) = &self.peek().kind {
                let msg = msg.clone();
                self.advance();
                self.record_error(format!("SyntaxError: {msg}"));
                break;
            }
            if self.peek().kind == TokenKind::Eof {
                break;
            }
            // TS 类型层声明（interface / type / declare）：与语句位同一套
            // 判定（含 `interface X extends A, B {}` 的继承子句与
            // `declare namespace N { ... }` 的环境体）
            if self.skip_ts_declaration() {
                continue;
            }
            body.push(self.parse_stmt());
        }
        Program { body }
    }

    /// 解析语句
    pub fn parse_stmt(&mut self) -> SpannedStmt {
        let line = self.cur_line();
        // TS 类型层声明在语句位一律剥离（块内同样适用——`declare namespace
        // N { interface X {} }` 的内层 interface 不再走语句解析）
        if self.skip_ts_declaration() {
            return Self::at(line, Stmt::Block(Vec::new()));
        }
        // `abstract class X {}`：`abstract` 是类型层修饰符，剥离后按普通
        // 类声明解析（`abstract` 作变量名时后随 `class` 才是该形态）
        if self.check_soft_keyword("abstract")
            && matches!(&self.peek_ahead(1).kind, TokenKind::Keyword(k) if k == "class")
        {
            self.advance();
        }
        if (self.peek().kind == TokenKind::Keyword("import".to_owned())
            || self.peek().kind == TokenKind::Ident("import".to_owned()))
            && !self.peek_ahead(1).is_punct("(")
            && !self.peek_ahead(1).is_punct(".")
        {
            return Self::at(line, self.parse_import_stmt());
        }

        if self.peek().kind == TokenKind::Keyword("export".to_owned())
            || self.peek().kind == TokenKind::Ident("export".to_owned())
        {
            return Self::at(line, self.parse_export_stmt());
        }

        // 空语句：`;` 独占语句位（零宽——编为空 Block，无指令、完成值
        // 链不受影响；此前无臂致 `{};{x: 42}` 的 `;` 落入表达式路径误报
        // "预期 ';'" SyntaxError）
        if self.match_punct(";") {
            return Self::at(line, Stmt::Block(Vec::new()));
        }

        if self.match_punct("{") {
            let mut stmts = Vec::new();
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                stmts.push(self.parse_stmt());
            }
            let _ = self.expect_punct("}");
            return Self::at(line, Stmt::Block(stmts));
        }

        // 标签语句：`Identifier : Statement`（`{length: 3000}` 块内为标签而非
        // 对象属性——此前无标签支持，`length` 被当表达式求值致 ReferenceError）
        if let TokenKind::Ident(label) = self.peek().kind.clone() {
            if self.peek_ahead(1).is_punct(":") {
                // async 函数体内 await 不得作标签（规范早错误）
                if self.in_async && label == "await" {
                    self.record_error("SyntaxError: async 函数中 await 不允许作标签".to_owned());
                }
                self.advance();
                self.advance();
                let body = Box::new(self.parse_stmt());
                return Self::at(line, Stmt::Labeled { label, body });
            }
        }

        if self.match_keyword("if") {
            let _ = self.expect_punct("(");
            let cond = self.parse_expr();
            let _ = self.expect_punct(")");
            let then_branch = Box::new(self.parse_stmt());
            let else_branch = if self.match_keyword("else") {
                Some(Box::new(self.parse_stmt()))
            } else {
                None
            };
            return Self::at(
                line,
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                },
            );
        }

        if self.match_keyword("while") {
            let _ = self.expect_punct("(");
            let cond = self.parse_expr();
            let _ = self.expect_punct(")");
            let body = Box::new(self.parse_stmt());
            return Self::at(line, Stmt::While { cond, body });
        }

        if self.match_keyword("with") {
            let _ = self.expect_punct("(");
            let obj = self.parse_expr();
            let _ = self.expect_punct(")");
            let body = Box::new(self.parse_stmt());
            return Self::at(line, Stmt::With { obj, body });
        }

        if self.match_keyword("do") {
            let body = Box::new(self.parse_stmt());
            let _ = self.match_keyword("while");
            let _ = self.expect_punct("(");
            let cond = self.parse_expr();
            let _ = self.expect_punct(")");
            self.eat_semi();
            return Self::at(line, Stmt::DoWhile { body, cond });
        }

        if self.match_keyword("for") {
            let is_await = self.match_keyword("await");
            let _ = self.expect_punct("(");

            let loop_kind = self.scan_for_loop_kind();
            if loop_kind == ForLoopKind::ForIn || loop_kind == ForLoopKind::ForOf {
                if self.peek().kind == TokenKind::Keyword("let".to_owned())
                    || self.peek().kind == TokenKind::Keyword("const".to_owned())
                    || self.peek().kind == TokenKind::Keyword("var".to_owned())
                {
                    self.advance();
                }
                let pattern = if self.check_punct("[") || self.check_punct("{") {
                    self.parse_var_pattern()
                } else if let TokenKind::Ident(id) = self.advance().kind {
                    VarPattern::Ident(id)
                } else {
                    VarPattern::Ident("anonymous".to_owned())
                };

                if loop_kind == ForLoopKind::ForIn {
                    let _ = self.expect_keyword("in");
                    let right = self.parse_expr();
                    let _ = self.expect_punct(")");
                    let body = Box::new(self.parse_stmt());
                    return Self::at(
                        line,
                        Stmt::ForIn {
                            pattern,
                            right,
                            body,
                        },
                    );
                } else {
                    if self.peek().kind == TokenKind::Keyword("of".to_owned())
                        || self.peek().kind == TokenKind::Ident("of".to_owned())
                    {
                        self.advance();
                    }
                    let right = self.parse_expr();
                    let _ = self.expect_punct(")");
                    let body = Box::new(self.parse_stmt());
                    return Self::at(
                        line,
                        Stmt::ForOf {
                            is_await,
                            pattern,
                            right,
                            body,
                        },
                    );
                }
            }

            let init = if self.match_punct(";") {
                None
            } else if self.peek().kind == TokenKind::Keyword("let".to_owned())
                || self.peek().kind == TokenKind::Keyword("var".to_owned())
                || self.peek().kind == TokenKind::Keyword("const".to_owned())
            {
                Some(Box::new(Self::at(line, self.parse_var_decl())))
            } else {
                let expr = self.parse_expr_sequence();
                let _ = self.expect_punct(";");
                Some(Box::new(Self::at(line, Stmt::Expr(expr))))
            };

            let cond = if self.check_punct(";") {
                self.advance();
                None
            } else {
                let c = self.parse_expr();
                let _ = self.expect_punct(";");
                Some(c)
            };

            let update = if self.check_punct(")") {
                None
            } else {
                Some(self.parse_expr_sequence())
            };
            let _ = self.expect_punct(")");
            let body = Box::new(self.parse_stmt());
            return Self::at(
                line,
                Stmt::For {
                    init,
                    cond,
                    update,
                    body,
                },
            );
        }

        if self.match_keyword("break") {
            // 可选标签：`break label;`（与 continue 同规——同行 Ident 才是
            // 标签；break 为受限产生式，换行后的 Ident 是下一语句）
            let label = if let TokenKind::Ident(id) = self.peek().kind.clone() {
                if !self.nl_before_current() {
                    self.advance();
                    Some(id)
                } else {
                    None
                }
            } else {
                None
            };
            self.eat_semi();
            return Self::at(line, Stmt::Break { label });
        }

        if self.match_keyword("continue") {
            // 可选标签：`continue label;`（同行 Ident 才是标签——continue 为
            // 受限产生式，换行后的 Ident 是**下一语句**的标识符，不得吞并）
            let label = if let TokenKind::Ident(id) = self.peek().kind.clone() {
                if !self.nl_before_current() {
                    self.advance();
                    Some(id)
                } else {
                    None
                }
            } else {
                None
            };
            self.eat_semi();
            return Self::at(line, Stmt::Continue { label });
        }

        if self.match_keyword("throw") {
            // 受限产生式：throw 与表达式之间不得出现行终止符
            if self.nl_before_current() {
                self.record_error("SyntaxError: throw 与表达式之间不允许换行".to_owned());
            }
            let expr = self.parse_expr();
            self.eat_semi();
            return Self::at(line, Stmt::Throw(expr));
        }

        if self.match_keyword("return") {
            // ASI（自动分号插入）：return 与后续代码之间出现换行时，语句在
            // return 处终止（ECMAScript restricted production）——npm 包无分号
            // 风格 `return\nif (...)` 依赖此规则；缺失会把下一行 if 当 return
            // 表达式解析（raw-body onEnd 实测错乱：err 检查丢失、done 无条件执行）
            let kw_start = self.tokens[self.pos - 1].start;
            let terminated = self.check_punct(";")
                || self.check_punct("}")
                || self.peek().kind == TokenKind::Eof
                || (self.pos < self.tokens.len()
                    && self.src[kw_start..self.peek().start].contains('\n'));
            let expr = if terminated {
                None
            } else {
                // 逗号序列合法（`return r && (n.x = r), n;` —— 压缩代码常见）
                Some(self.parse_expr_sequence())
            };
            self.eat_semi();
            return Self::at(line, Stmt::Return(expr));
        }

        if self.match_keyword("try") {
            let body = Box::new(self.parse_stmt());
            let mut catch_param = None;
            let mut catch_body = None;
            if self.match_keyword("catch") {
                if self.match_punct("(") {
                    if let TokenKind::Ident(id) = self.peek().kind.clone() {
                        self.advance();
                        catch_param = Some(id);
                        self.skip_type_annotation();
                    }
                    let _ = self.expect_punct(")");
                }
                catch_body = Some(Box::new(self.parse_stmt()));
            }
            let finally_body = if self.match_keyword("finally") {
                Some(Box::new(self.parse_stmt()))
            } else {
                None
            };
            return Self::at(
                line,
                Stmt::Try {
                    body,
                    catch_param,
                    catch_body,
                    finally_body,
                },
            );
        }

        if self.peek().kind == TokenKind::Keyword("let".to_owned())
            || self.peek().kind == TokenKind::Keyword("const".to_owned())
            || self.peek().kind == TokenKind::Keyword("var".to_owned())
        {
            return Self::at(line, self.parse_var_decl());
        }

        if self.match_keyword("async") {
            // 规范 AsyncFunctionDeclaration：async 与 function **同行**才
            // 构成修饰符——换行后 function 是下一语句（ASI），async 自身
            // 是标识符表达式语句（`async` 单独一行后跟 function 声明时
            // 运行时 ReferenceError: async is not defined）
            if !self.nl_before_current() && self.match_keyword("function") {
                let is_generator = self.match_punct("*");
                let mut def = self.parse_function_def(true, is_generator);
                def.is_async = true;
                def.is_generator = is_generator;
                return Self::at(line, Stmt::Function(def));
            }
            self.pos -= 1;
        }

        if self.match_keyword("function") {
            let is_generator = self.match_punct("*");
            let mut def = self.parse_function_def(false, is_generator);
            def.is_generator = is_generator;
            return Self::at(line, Stmt::Function(def));
        }

        if self.match_keyword("class") {
            return Self::at(line, self.parse_class_stmt());
        }

        if self.match_keyword("switch") {
            let _ = self.expect_punct("(");
            let discriminant = self.parse_expr();
            let _ = self.expect_punct(")");
            let _ = self.expect_punct("{");
            let mut cases = Vec::new();
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                if self.match_keyword("case") {
                    let test = self.parse_expr();
                    let _ = self.expect_punct(":");
                    let mut consequent = Vec::new();
                    while !self.check_keyword("case")
                        && !self.check_keyword("default")
                        && !self.check_punct("}")
                        && self.peek().kind != TokenKind::Eof
                    {
                        consequent.push(self.parse_stmt());
                    }
                    cases.push(SwitchCase {
                        test: Some(test),
                        consequent,
                    });
                } else if self.match_keyword("default") {
                    let _ = self.expect_punct(":");
                    let mut consequent = Vec::new();
                    while !self.check_keyword("case")
                        && !self.check_keyword("default")
                        && !self.check_punct("}")
                        && self.peek().kind != TokenKind::Eof
                    {
                        consequent.push(self.parse_stmt());
                    }
                    cases.push(SwitchCase {
                        test: None,
                        consequent,
                    });
                } else {
                    self.advance();
                }
            }
            let _ = self.expect_punct("}");
            return Self::at(
                line,
                Stmt::Switch {
                    discriminant,
                    cases,
                },
            );
        }

        // 默认作为表达式语句（逗号运算符序列：`a = 1, b = 2;` 是单个
        // 逗号表达式——此前 parse_expr 停在逗号、宽松 eat_semi 以
        // 「拆成多条语句」掩盖，ASI 严格化后必须整串解析）
        let expr = self.parse_expr_sequence();
        // 裸 Ident 表达式语句：仅当下一 token 是**关键字**时宽松吞分号
        //（TS `declare enum` 等 strip-only 豁免形态依赖）；裸 Ident 后同行
        // 跟 Ident（`line comment`，无行终结符）不得 ASI——必须报
        // SyntaxError（Node 22 实测 "Unexpected identifier"）
        let bare_ident = matches!(&expr, Expr::Ident(_));
        // 裸 Ident 宽容仅限 TS strip-only 形态链（`declare enum Color {..}`）：
        // 下一 token 是关键字 / TS 标记 Ident（enum/namespace/type，非保留字
        // 故词法为 Ident）/ `{`（enum 体前导的绑定名形态）；裸 Ident 后同行
        // 跟普通 Ident（`line comment`，无行终结符）不得 ASI——必须报
        // SyntaxError（Node 22 实测 "Unexpected identifier"）
        let next_is_ts_marker = matches!(&self.peek().kind, TokenKind::Keyword(_))
            || matches!(&self.peek().kind, TokenKind::Ident(id) if matches!(id.as_str(), "enum" | "namespace" | "type"))
            || self.check_punct("{");
        let expr_is_ts_marker = matches!(&expr, Expr::Ident(id) if matches!(id.as_str(), "enum" | "namespace" | "type" | "declare"));
        if bare_ident && (next_is_ts_marker || expr_is_ts_marker) {
            self.match_punct(";");
        } else {
            self.eat_semi();
        }
        Self::at(line, Stmt::Expr(expr))
    }

    /// 试探解构赋值目标：`[`/`{` 起，先向前扫描找配对闭合符并确认其后紧跟
    /// `=`；命中则以 `parse_var_pattern` 解析模式并返回（游标停在 `=` 前）。
    fn try_parse_destructure_assign_target(&mut self) -> Option<VarPattern> {
        let save = self.pos;
        let open = if self.check_punct("[") { "[" } else { "{" };
        let close = if open == "[" { "]" } else { "}" };
        let mut depth = 0i32;
        let mut i = self.pos;
        let mut found = None;
        while i < self.tokens.len() {
            if let TokenKind::Punct(p) = &self.tokens[i].kind {
                if p == open {
                    depth += 1;
                } else if p == close {
                    depth -= 1;
                    if depth == 0 {
                        found = Some(i);
                        break;
                    }
                }
            }
            i += 1;
        }
        let end = found?;
        // 闭合符之后须紧跟 `=`（排除 `==`/`=>`）
        let next = self.tokens.get(end + 1)?;
        if !next.is_punct("=") {
            return None;
        }
        let pat = self.parse_var_pattern();
        if self.match_punct("=") {
            Some(pat)
        } else {
            self.pos = save;
            None
        }
    }

    fn parse_var_pattern(&mut self) -> VarPattern {
        if self.match_punct("[") {
            let mut elements = Vec::new();
            while !self.check_punct("]") && self.peek().kind != TokenKind::Eof {
                // 空元素位（`[a,, b]`）：**产生洞元素**并占一个源索引。
                // 此前直接 continue 不记录，致 `[a,,b]=[1,2,undefined,4,5]`
                // 的 b 取到索引 2（应为 3）；洞与 rest 的起始偏移同样错位。
                if self.check_punct(",") {
                    self.advance();
                    elements.push(ArrayPatternElem {
                        name: String::new(),
                        is_rest: false,
                        default_value: None,
                        is_hole: true,
                    });
                    continue;
                }
                if self.match_punct("...") {
                    // rest 后可跟**嵌套模式**（`...[...[]]` / `...[a, ...c]`）：
                    // `[`/`{` 走嵌套模式解析取占位名（不得误当 Ident 消耗
                    // 起始括号——此前 `[` 被吞致括号失衡 SyntaxError）
                    let name = if self.check_punct("[") || self.check_punct("{") {
                        let nested = self.parse_var_pattern();
                        match &nested {
                            VarPattern::Object(props) => {
                                props.first().map(|p| p.key.clone()).unwrap_or_default()
                            }
                            VarPattern::Array(els) => {
                                els.first().map(|e| e.name.clone()).unwrap_or_default()
                            }
                            VarPattern::Ident(n) => n.clone(),
                        }
                    } else if let TokenKind::Ident(id) = self.advance().kind {
                        id
                    } else {
                        String::new()
                    };
                    let default_value = if self.match_punct("=") {
                        Some(self.parse_expr())
                    } else {
                        None
                    };
                    elements.push(ArrayPatternElem {
                        name,
                        is_rest: true,
                        default_value,
                        is_hole: false,
                    });
                    break;
                } else if self.check_punct("[") || self.check_punct("{") {
                    // 嵌套解构模式（`[x, {y}, ...z]`）：绑定名取占位，
                    // 嵌套模式登记到 VarPattern::Array 的 name 文本旁路
                    let nested = self.parse_var_pattern();
                    let nested_name = match &nested {
                        VarPattern::Object(props) => {
                            props.first().map(|p| p.key.clone()).unwrap_or_default()
                        }
                        VarPattern::Array(els) => {
                            els.first().map(|e| e.name.clone()).unwrap_or_default()
                        }
                        VarPattern::Ident(n) => n.clone(),
                    };
                    let default_value = if self.match_punct("=") {
                        Some(self.parse_expr())
                    } else {
                        None
                    };
                    elements.push(ArrayPatternElem {
                        name: nested_name,
                        is_rest: false,
                        default_value,
                        is_hole: false,
                    });
                } else if let TokenKind::Ident(id) = self.advance().kind {
                    let default_value = if self.match_punct("=") {
                        Some(self.parse_expr())
                    } else {
                        None
                    };
                    elements.push(ArrayPatternElem {
                        name: id,
                        is_rest: false,
                        default_value,
                        is_hole: false,
                    });
                }
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct("]");
            VarPattern::Array(elements)
        } else if self.match_punct("{") {
            let mut props = Vec::new();
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                // 对象 rest：`{ a, b: v, ...rest }`。规范形态仅允许标识符
                // 绑定（禁止嵌套模式与默认值），且必为最后一项。
                if self.match_punct("...") {
                    let name = if let TokenKind::Ident(id) = self.advance().kind {
                        id
                    } else {
                        String::new()
                    };
                    props.push(ObjectPatternProp {
                        key: name.clone(),
                        value: VarPattern::Ident(name),
                        default_value: None,
                        is_rest: true,
                    });
                    break;
                }
                let key = if let TokenKind::Ident(id) = self.advance().kind {
                    id
                } else {
                    String::new()
                };
                let value = if self.match_punct(":") {
                    self.parse_var_pattern()
                } else {
                    VarPattern::Ident(key.clone())
                };
                let default_value = if self.match_punct("=") {
                    Some(self.parse_expr())
                } else {
                    None
                };
                props.push(ObjectPatternProp {
                    key,
                    value,
                    default_value,
                    is_rest: false,
                });
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct("}");
            VarPattern::Object(props)
        } else if let TokenKind::Ident(id) = self.advance().kind {
            VarPattern::Ident(id)
        } else {
            VarPattern::Ident("anonymous".to_owned())
        }
    }

    fn parse_var_decl(&mut self) -> Stmt {
        let kind = match &self.peek().kind {
            TokenKind::Keyword(k) => match k.as_str() {
                "let" => VarKind::Let,
                "const" => VarKind::Const,
                _ => VarKind::Var,
            },
            _ => VarKind::Var,
        };
        self.advance(); // 跳过 let / const / var
        if self.check_punct("[") || self.check_punct("{") {
            let pattern = self.parse_var_pattern();
            let _ = self.expect_punct("=");
            let init = self.parse_expr();
            self.eat_semi();
            return Stmt::DestructureDecl { pattern, init };
        }

        // `from` 为上下文关键字（import ... from），可作合法变量名
        // （mime-types 等真实包存在 `var from = ...`）；`yield` 在非生成器
        // 语境同为普通标识符（`var yield = 'y'` + 计算访问器键
        // `get [yield]()` 语料形态）
        // 经 advance_ident_like（含上下文关键字；`await` 在 async 语境被
        // context_ident 拒绝，故 `async function f(){ var await; }` 报错）
        let name = match self.advance_ident_like() {
            Some(n) => n,
            None => {
                let other = self.peek().kind.clone();
                let message = format!("var/let/const 声明缺少变量名，实为 {other:?}");
                self.record_error(message);
                "anonymous".to_owned()
            }
        };
        self.skip_type_annotation();
        let init = if self.match_punct("=") {
            Some(self.parse_expr())
        } else {
            None
        };
        // 多声明符：`var i = 0, len = expr;`（for-init 常见形态）
        let mut extra: Vec<(String, Option<Expr>)> = Vec::new();
        while self.match_punct(",") {
            // 多声明符的名字同样接受上下文关键字（`var a = x, async = y`
            // —— asynckit/terminator.js 的既有写法）
            let extra_name = match self.advance_ident_like() {
                Some(n) => n,
                None => {
                    let other = self.peek().kind.clone();
                    let message = format!("var/let/const 声明缺少变量名，实为 {other:?}");
                    self.record_error(message);
                    "anonymous".to_owned()
                }
            };
            self.skip_type_annotation();
            let extra_init = if self.match_punct("=") {
                Some(self.parse_expr())
            } else {
                None
            };
            extra.push((extra_name, extra_init));
        }
        self.eat_semi();
        if extra.is_empty() {
            Stmt::VarDecl { name, init, kind }
        } else {
            let mut decls = vec![(name, init)];
            decls.extend(extra);
            Stmt::MultiVarDecl { kind, decls }
        }
    }

    fn parse_function_def(&mut self, is_async: bool, is_generator: bool) -> FunctionDef {
        // 函数名可为标识符或上下文关键字（`function async(cb) {}` ——
        // asynckit/axios 等真实包的既有写法；async 在非函数表达式前缀位置
        // 是普通标识符）
        let name = if let Some(id) = self.advance_ident_like() {
            // async 函数绑定名不得为 arguments/eval（规范早错误；
            // `async function arguments() {}` → SyntaxError）
            if is_async && matches!(id.as_str(), "arguments" | "eval") {
                self.record_error(format!("SyntaxError: async 函数名不允许为 {id}"));
            }
            id
        } else {
            String::new()
        };
        // TS 类型形参：`function id<T>(v: T): T`
        let _ = self.try_skip_type_args();
        let _ = self.expect_punct("(");
        let mut params = Vec::new();
        let mut is_var_args = false;
        let mut saw_default = false;
        let mut prologue_stmts = Vec::new();
        let outer_async = self.in_async;
        self.in_async = is_async;
        // 生成器语境：体内 `yield` 为生成器运算符（存续到体解析结束）
        let outer_generator = self.in_generator;
        self.in_generator = is_generator;
        // 普通函数无 HomeObject：形参默认值与函数体内 super 均 SyntaxError
        let outer_super = self.super_disallowed;
        self.super_disallowed = true;

        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
            let is_rest_iter = self.match_punct("...");
            if is_rest_iter {
                is_var_args = true;
            } else if is_var_args {
                // rest 参数后不得再有任何形参（`...a, b` → SyntaxError）
                self.record_error("SyntaxError: rest 参数之后不允许再有形参".to_owned());
            }
            if self.check_punct("[") || self.check_punct("{") {
                let pattern = self.parse_var_pattern();
                let param_name = format!("__param_{}__", params.len());
                params.push(param_name.clone());
                let dline = self.cur_line();
                // 解构形参的**默认值**（`function f({a} = {}) {}`——axios 等
                // 真实包大量使用）：`= expr` 时 init 取条件表达式
                //（param === undefined ? expr : param）
                let init = if self.match_punct("=") {
                    saw_default = true;
                    let def = self.parse_expr();
                    Expr::Conditional {
                        cond: Box::new(Expr::Binary {
                            op: "===".to_owned(),
                            left: Box::new(Expr::Ident(param_name.clone())),
                            right: Box::new(Expr::Undefined),
                        }),
                        then_expr: Box::new(def),
                        else_expr: Box::new(Expr::Ident(param_name.clone())),
                    }
                } else {
                    Expr::Ident(param_name)
                };
                prologue_stmts.push(Self::at(dline, Stmt::DestructureDecl { pattern, init }));
                self.skip_type_annotation();
            } else if matches!(self.peek().kind, TokenKind::Keyword(ref k) if k == "await")
                && is_async
            {
                // async 函数形参名不得为 await（await 为 Keyword token，
                // 走不到下方 Ident 臂——需单独拦截）
                self.advance();
                self.record_error("SyntaxError: async 函数形参名不允许为 await".to_owned());
            } else if self.check_this_param() {
                // `this: T` 伪形参（TS）：仅类型占位，剥离后不占实参位
                self.advance();
                self.skip_type_annotation();
            } else if let Some(param_name) = self.advance_ident_like() {
                // 形参名走 advance_ident_like：上下文关键字（from/of/as/
                // get/set/static/async 等）是普通标识符——color-convert
                // `function link(from, to)` 实测依赖
                // async 形参名不得为 arguments/eval（规范早错误）；
                // strict 语义下**所有**函数形参名均不得为 arguments/eval
                //（StrictFormalParameters——onlyStrict 变体负例族）
                if (is_async || self.strict) && matches!(param_name.as_str(), "arguments" | "eval")
                {
                    self.record_error(format!("SyntaxError: 形参名不允许为 {param_name}"));
                }
                params.push(param_name.clone());
                self.skip_param_suffix();
                // 默认参数 `param = default`：运行时参数为 undefined 时取默认值
                //（对齐 Go 前端：函数体 prologue 注入条件赋值）
                if self.match_punct("=") {
                    if is_var_args {
                        // rest 参数带默认值 → SyntaxError
                        self.record_error("SyntaxError: rest 参数不允许有默认值".to_owned());
                    }
                    saw_default = true;
                    let default_expr = self.parse_expr();
                    let pline = self.cur_line();
                    prologue_stmts.push(Self::at(
                        pline,
                        Stmt::Expr(Expr::Assign {
                            name: param_name.clone(),
                            value: Box::new(Expr::Conditional {
                                cond: Box::new(Expr::Binary {
                                    op: "===".to_owned(),
                                    left: Box::new(Expr::Ident(param_name.clone())),
                                    right: Box::new(Expr::Undefined),
                                }),
                                then_expr: Box::new(default_expr),
                                else_expr: Box::new(Expr::Ident(param_name)),
                            }),
                        }),
                    ));
                }
            }
            if self.match_punct(",") {
                if is_var_args && self.check_punct(")") {
                    // rest 参数后不允许尾逗号（`...a,)` → SyntaxError）
                    self.record_error("SyntaxError: rest 参数后不允许尾逗号".to_owned());
                }
                continue;
            }
            break;
        }
        let _ = self.expect_punct(")");
        self.skip_type_annotation(); // 函数返回值类型
        let body_stmt = self.parse_stmt();
        let mut body = match body_stmt {
            SpannedStmt {
                stmt: Stmt::Block(stmts),
                ..
            } => stmts,
            other => vec![other],
        };
        // 规范：非简单参数列表（解构/默认/剩余）的函数体不得含
        // "use strict" 指令（SyntaxError；M7.2 官方语料 async-function
        // 语法族 ~28 例）；非简单列表形参名亦不得重复
        let non_simple = !prologue_stmts.is_empty() || is_var_args || saw_default;
        if non_simple {
            let mut seen = std::collections::HashSet::new();
            for p in &params {
                if !seen.insert(p.clone()) {
                    self.record_error(format!("SyntaxError: 非简单参数列表的形参名重复（{p}）"));
                }
            }
            let has_use_strict = body.iter().take_while(|s| Self::is_directive(s)).any(|s| {
                matches!(
                    &s.stmt,
                    Stmt::Expr(Expr::String(d)) if d == "use strict"
                )
            });
            if has_use_strict {
                self.record_error(
                    "SyntaxError: 非简单参数列表的函数体不允许 use strict 指令".to_owned(),
                );
            }
        } else {
            // 简单参数列表：形参名重复仅 strict 语义报错
            //（StrictFormalParameters——async/普通函数声明同规）
            let mut seen = std::collections::HashSet::new();
            if params.iter().any(|p| !seen.insert(p.clone())) {
                let body_strict = body.iter().take_while(|s| Self::is_directive(s)).any(|s| {
                    matches!(
                        &s.stmt,
                        Stmt::Expr(Expr::String(d)) if d == "use strict"
                    )
                });
                if self.strict || body_strict {
                    self.record_error("SyntaxError: strict 模式下简单形参名不得重复".to_owned());
                }
            }
        }
        // async 生成器边界标记：参数默认值求值完成后、函数体开始前注入
        // `yield;`——创建时的一次驱动停在标记处（体未开始、默认值抛错
        // 已同步传播）；首个 next() 从体起点恢复
        if is_generator && is_async {
            let marker_line = self.cur_line();
            let marker = Self::at(
                marker_line,
                Stmt::Expr(Expr::Yield {
                    value: None,
                    delegate: false,
                }),
            );
            if !prologue_stmts.is_empty() {
                // 标记插在参数 prologue **之后**（默认值求值仍属调用时语义）
                prologue_stmts.push(marker);
                prologue_stmts.append(&mut body);
                body = prologue_stmts;
            } else {
                body.insert(0, marker);
            }
        } else if !prologue_stmts.is_empty() {
            prologue_stmts.append(&mut body);
            body = prologue_stmts;
        }
        self.in_async = outer_async;
        self.in_generator = outer_generator;
        self.super_disallowed = outer_super;
        // 体**顶层** let/const 不得与形参重名（`foo(bar){ let bar; }` →
        // SyntaxError；嵌套块内 let 遮蔽合法，不在此查）
        for s in &body {
            let conflict = match &s.stmt {
                Stmt::VarDecl {
                    name,
                    kind: VarKind::Let | VarKind::Const,
                    ..
                } => params.contains(name),
                Stmt::MultiVarDecl {
                    kind: VarKind::Let | VarKind::Const,
                    decls,
                    ..
                } => decls.iter().any(|(n, _)| params.contains(n)),
                _ => false,
            };
            if conflict {
                self.record_error("SyntaxError: 体顶层 let/const 不得与形参重名".to_owned());
                break;
            }
        }
        FunctionDef {
            name,
            params,
            is_var_args,
            body,
            is_async: false,
            is_generator: false,
            is_arrow: false,
        }
    }

    fn parse_class_stmt(&mut self) -> Stmt {
        let name = if let TokenKind::Ident(id) = self.peek().kind.clone() {
            self.advance();
            id
        } else {
            "AnonymousClass".to_owned()
        };
        // TS 类型形参：`class Store<T extends Entity> extends Base`
        let _ = self.try_skip_type_args();
        let (super_class, constructor, methods, class_fields) = self.parse_class_tail(Some(&name));
        Stmt::Class {
            name,
            super_class: super_class.map(|b| *b),
            constructor,
            methods,
            fields: class_fields,
        }
    }

    /// 类表达式：`class Name? { ... }` 出现在表达式位（typebox 的
    /// `return class { constructor() {...} }` 依赖；匿名类绑定名为 None）。
    fn parse_class_expr(&mut self) -> Expr {
        let name = if let TokenKind::Ident(id) = self.peek().kind.clone() {
            self.advance();
            Some(id)
        } else {
            None
        };
        let _ = self.try_skip_type_args();
        let (super_class, constructor, methods, class_fields) =
            self.parse_class_tail(name.as_deref());
        Expr::Class {
            name,
            super_class,
            constructor,
            methods,
            fields: class_fields,
        }
    }

    /// `class` 关键字之后的共享尾部（extends 子句 + 类体），语句/表达式
    /// 两种形态共用；`name` 为 None 时按匿名类解析。
    fn parse_class_tail(&mut self, name: Option<&str>) -> ClassTail {
        let name_str = name.unwrap_or("");
        let super_class = if self.match_keyword("extends") {
            // 父类表达式可含成员访问（`class D extends ns.Base {}` ——
            // axios/agent-base 等真实包的既有写法）；`parse_expr_primary`
            // 只解析主表达式，故改用 unary 层级（含 Member/Call 后缀链）
            Some(Box::new(self.parse_unary()))
        } else {
            None
        };
        // `class C extends Base<T>` 的父类泛型实参（`<` 在表达式位是
        // 小于号，故仅在「`>` 后紧接 `{`/`implements`」时按类型实参收口）
        if self.check_punct("<") {
            let _ = self.try_skip_type_args();
        }
        self.skip_implements_clause();

        let _ = self.expect_punct("{");
        let mut constructor = None;
        let mut methods = Vec::new();
        // 类体方法有 HomeObject：super 合法——清除普通函数的禁用旗标
        let outer_super = self.super_disallowed;
        self.super_disallowed = false;

        let mut class_fields: Vec<(String, bool, Option<Expr>)> = Vec::new();
        while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
            // TS 成员修饰符（`public`/`private`/`protected`/`readonly`/
            // `abstract`/`declare`/`override`/`static`）是软关键字：仅当
            // 后随另一修饰符或成员键时才按修饰符消耗（`private = 1` 是名为
            // private 的字段）。`abstract`/`declare` 成员按 strip-only 语义
            // **整体擦除**，不生成原型方法或实例字段。
            let (is_static, is_erased_member) = self.take_member_modifiers();
            if is_erased_member {
                self.skip_erased_member();
                continue;
            }
            // 类字段（`field = 1;` / `static s = 2;` / `field;`）：以
            // `__class_field_<name>` 子语句收集，装配期注入构造器
            // （实例字段 `this.name = init` / 静态字段挂构造器）
            {
                let save = self.pos;
                let is_static_field = is_static;
                let mut fname = String::new();
                if self.check_punct("#") {
                    self.advance();
                    if let TokenKind::Ident(id) = self.peek().kind.clone() {
                        self.advance();
                        fname = format!("#{id}");
                    }
                } else if let Some(n) = self.advance_ident_like() {
                    fname = n;
                } else if let TokenKind::String(s) = self.peek().kind.clone() {
                    self.advance();
                    fname = s;
                } else if let TokenKind::Number(n) = self.peek().kind.clone() {
                    self.advance();
                    fname = format!("{n}");
                }
                // TS 字段的确定赋值/可选标记与类型注解：`n!: number;` /
                // `n?: T;` / `n: T = init;`（方法名后随 `(` 时不进入本分支）
                if !fname.is_empty() && !self.check_punct("(") {
                    let _ = self.match_punct("!");
                    let _ = self.match_punct("?");
                    self.skip_type_annotation();
                }
                // 字段终止符：`=`（带初值）/ `;` / `}`（体尾无分号——
                // `class C { x = 1 }` 与 `class C { x?: T }`）
                if !fname.is_empty()
                    && !self.check_punct("(")
                    && (self.check_punct("=") || self.check_punct(";") || self.check_punct("}"))
                {
                    let init = if self.match_punct("=") {
                        Some(self.parse_expr())
                    } else {
                        None
                    };
                    self.eat_semi();
                    class_fields.push((fname, is_static_field, init));
                    if !self.check_punct("}") {
                        continue;
                    }
                    break;
                }
                self.pos = save;
            }
            // `static` 非保留字（词法为 Ident）：仅在后随键名/访问器/计算键
            // 时作修饰符——`static() {}` 是名为 static 的普通方法。上方
            // take_member_modifiers 已消耗 TS 组合里的 static，此处兜底
            // 未被其收下的形态。
            let is_static = is_static
                || (matches!(&self.peek().kind, TokenKind::Ident(s) if s == "static")
                    && !self.peek_ahead(1).is_punct("(")
                    && {
                        self.advance();
                        true
                    });
            // async 方法前缀：`async m() {}` / `async *m() {}`（axios 的
            // AxiosHeaders 等类大量使用；async 仅在**后随方法名/`*`** 时
            // 作修饰符——`async() {}` 是名为 async 的普通方法）
            let mut m_is_async = false;
            if self.check_keyword("async")
                && !self.peek_ahead(1).is_punct("(")
                && !self.peek_ahead(1).is_punct("=")
                && !self.peek_ahead(1).is_punct(";")
                && !self.peek_ahead(1).is_punct("}")
                && !self.peek_ahead(1).is_punct(":")
                && !self.peek_ahead(1).is_punct(",")
                && !self.peek_ahead(1).is_punct("=>")
            {
                self.advance();
                m_is_async = true;
            }
            // 生成器前缀：`*m() {}` / `*['constructor']()`（`*` 后须跟随键名
            // 或计算键；不识别会令类体 while 无进展挂死——generator 静态族）
            let is_generator = self.check_punct("*") && !self.peek_ahead(1).is_punct("(") && {
                self.advance();
                true
            };
            // 访问器前缀：`get x() {}` / `set x(v) {}`（仅当后随键名而非 `(`）
            let mut accessor_kind = 0u32;
            if let TokenKind::Ident(prefix) = self.peek().kind.clone() {
                if (prefix == "get" || prefix == "set") && !self.peek_ahead(1).is_punct("(") {
                    self.advance();
                    accessor_kind = if prefix == "get" { 1 } else { 2 };
                }
            }
            // 计算键（`['constructor']() {}`）不是 `constructor` 方法——
            // 规范仅**字面量**名 constructor 定义构造器（计算键方法挂原型，
            // `C.prototype.constructor` 保持回指构造器）
            let mut is_computed_key = false;
            let m_name = if let TokenKind::Ident(id) = self.peek().kind.clone() {
                self.advance();
                id
            } else if let TokenKind::Keyword(kw) = self.peek().kind.clone() {
                self.advance();
                kw
            } else if let TokenKind::String(s) = self.peek().kind.clone() {
                // 字符串字面量成员名（`"~validate"(data) {}` / `get "x"() {}` /
                // `static "z"() {}`）：键为字面量文本，**不是**计算键
                // （zod v3 types.cjs 的 `"~validate"(data)` 依赖此形态）
                self.advance();
                s
            } else if let TokenKind::Number(n) = self.peek().kind.clone() {
                // 数值字面量成员名（`1() {}` / `get 2() {}`）：按数字文本作键
                self.advance();
                format!("{n}")
            } else if self.check_punct("#") {
                // 私有成员（`#p = 3` / `#m() {}`）：以 `#名` 作为成员名
                // （VM 侧按普通属性存储——私有性的强校验未实现，登记为近似）
                self.advance();
                if let TokenKind::Ident(id) = self.peek().kind.clone() {
                    self.advance();
                    format!("#{id}")
                } else {
                    String::new()
                }
            } else if self.check_punct("[") {
                // 计算键：`get ['a']() {}` / `[Symbol.iterator]() {}`——
                // **解析完整表达式**后取其静态名（成员表达式 `a.b` 取文本
                // 尾名；字面量取字面值）。此前只取首个 token，致
                // `[Symbol.iterator]` 落为 "Symbol"。
                self.advance();
                let key_expr = self.parse_expr();
                let mut well_known: Option<String> = None;
                let key_txt = match &key_expr {
                    // 知名符号键：`[Symbol.iterator]` 压缩为 "@@iterator"
                    // 文本名，由 VM 类装配期还原为符号的 mangled 键——
                    // 迭代协议/toStringTag 等按符号查找，字符串键永远命中
                    // 不了。其余计算键维持「栈传值」形态（kind 0x20）。
                    Expr::Member { obj, prop }
                        if matches!(obj.as_ref(), Expr::Ident(id) if id == "Symbol")
                            && matches!(
                                prop.as_str(),
                                "iterator"
                                    | "asyncIterator"
                                    | "hasInstance"
                                    | "isConcatSpreadable"
                                    | "match"
                                    | "replace"
                                    | "search"
                                    | "species"
                                    | "split"
                                    | "toPrimitive"
                                    | "toStringTag"
                                    | "unscopables"
                            ) =>
                    {
                        well_known = Some(prop.clone());
                        format!("@@{prop}")
                    }
                    Expr::String(s) => s.clone(),
                    Expr::Number(n) => format!("{n}"),
                    Expr::Ident(id) => id.clone(),
                    Expr::Member { prop, .. } => prop.clone(),
                    other => format!("{other:?}"),
                };
                let _ = self.expect_punct("]");
                if well_known.is_some() {
                    // 压缩形态不经栈传值：按普通（文本）键装配
                    is_computed_key = false;
                } else {
                    is_computed_key = true;
                }
                key_txt
            } else {
                break;
            };

            // 泛型方法：`get<T>(k: string): T {}`
            let _ = self.try_skip_type_args();
            let _ = self.expect_punct("(");
            let mut params = Vec::new();
            let mut is_var_args = false;
            // 形参默认值降级：与具名函数同款——体前注入
            // `param = (param === undefined ? 默认值 : param)` 条件赋值。
            // 此前只「消费语法」，默认值从未生效（`constructor(a, b = {})`
            // 缺省实参时 b 为 undefined）。
            let mut prologue_stmts: Vec<SpannedStmt> = Vec::new();
            while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
                // rest 参数（`concat(...targets) {}`——axios 的 AxiosHeaders
                // 等类方法大量使用）
                if self.match_punct("...") {
                    is_var_args = true;
                }
                if self.check_this_param() {
                    self.advance();
                    self.skip_type_annotation();
                } else if let Some(p) = self.advance_ident_like() {
                    params.push(p.clone());
                    self.skip_param_suffix();
                    if self.match_punct("=") {
                        let default_expr = self.parse_expr();
                        let pline = self.cur_line();
                        prologue_stmts.push(Self::at(
                            pline,
                            Stmt::Expr(Expr::Assign {
                                name: p.clone(),
                                value: Box::new(Expr::Conditional {
                                    cond: Box::new(Expr::Binary {
                                        op: "===".to_owned(),
                                        left: Box::new(Expr::Ident(p.clone())),
                                        right: Box::new(Expr::Undefined),
                                    }),
                                    then_expr: Box::new(default_expr),
                                    else_expr: Box::new(Expr::Ident(p.clone())),
                                }),
                            }),
                        ));
                    }
                }
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct(")");
            self.skip_type_annotation();
            // 类方法体的生成器/async 语境：`*g() { yield v; }` 体内 yield
            // 须被识别（`async *encode()` 同规）——此前未置位致「yield 带
            // 操作数」在类方法体内解析失败（axios 的 FormDataPart 形态）
            let outer_gen = self.in_generator;
            let outer_async = self.in_async;
            self.in_generator = is_generator;
            self.in_async = m_is_async;
            let body_stmt = self.parse_stmt();
            self.in_generator = outer_gen;
            self.in_async = outer_async;
            let mut body = match body_stmt {
                SpannedStmt {
                    stmt: Stmt::Block(stmts),
                    ..
                } => stmts,
                other => vec![other],
            };
            if !prologue_stmts.is_empty() {
                prologue_stmts.append(&mut body);
                body = prologue_stmts;
            }

            if m_name == "constructor" && !is_computed_key {
                constructor = Some(FunctionDef {
                    name: format!("{name_str}_constructor"),
                    params,
                    is_var_args,
                    body,
                    is_async: false,
                    is_generator: false,
                    is_arrow: false,
                });
            } else {
                methods.push(ClassMethodDef {
                    name: m_name,
                    params,
                    body,
                    is_static,
                    is_generator,
                    is_var_args,
                    is_async: m_is_async,
                    // 高位 0x20：计算键标记（跨 bytecode 传给 VM 的早错误判定）
                    kind: accessor_kind | if is_computed_key { 0x20 } else { 0 },
                    is_computed: is_computed_key,
                });
            }
        }
        let _ = self.expect_punct("}");
        self.super_disallowed = outer_super;

        (super_class, constructor, methods, class_fields)
    }

    /// 解析表达式入口
    pub fn parse_expr(&mut self) -> Expr {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Expr {
        // 生成器语境的 `yield` 才是生成器运算符；非生成器语境 `yield`
        // 是普通标识符（此前顶层/普通函数内 `get [yield]()` 被误解析为
        // yield 运算符——挂起信号逃逸到顶层致 VM 报错）
        if self.check_keyword("yield") && self.in_generator {
            self.advance();
            let delegate = self.match_punct("*");
            let value = if !self.check_punct(";")
                && !self.check_punct(")")
                && !self.check_punct("]")
                && !self.check_punct("}")
                && !self.check_punct(",")
                && self.peek().kind != TokenKind::Eof
            {
                Some(Box::new(self.parse_assignment()))
            } else {
                None
            };
            return Expr::Yield { value, delegate };
        }

        // 解构赋值（`[a,b] = v` / `({x} = v)`）：`[`/`{` 开头时先向前扫描
        // 匹配到对应的 `]`/`}` 之后是否跟 `=`——是则按模式解析（目标写回既有
        // 绑定），否则回退常规字面量解析。语句首的 `{` 已被 parse_stmt 的块
        // 分支消化，此处只可能是表达式位置的模式。
        if (self.check_punct("[") || self.check_punct("{"))
            && let Some(pat) = self.try_parse_destructure_assign_target()
        {
            let val = self.parse_assignment();
            return Expr::DestructureAssign {
                pattern: pat,
                init: Box::new(val),
            };
        }

        let expr = self.parse_conditional();
        if self.match_punct("=") {
            let val = self.parse_assignment();
            return match expr {
                Expr::Ident(name) => Expr::Assign {
                    name,
                    value: Box::new(val),
                },
                Expr::Member { obj, prop } => Expr::MemberAssign {
                    obj,
                    prop,
                    value: Box::new(val),
                },
                Expr::Index { obj, index } => Expr::IndexAssign {
                    obj,
                    index,
                    value: Box::new(val),
                },
                // 赋值目标为字面量（`true = 1`、`1 = x`、`"s" = y` 等）：
                // 规范 Invalid left-hand side → SyntaxError（S8.x 负例族；
                // 此前静默丢弃赋值右侧）
                Expr::Boolean(_)
                | Expr::Null
                | Expr::Number(_)
                | Expr::BigInt(_)
                | Expr::String(_) => {
                    self.record_error("SyntaxError: 赋值目标不能为字面量".to_owned());
                    expr
                }
                other => other,
            };
        }

        if let TokenKind::Punct(p) = &self.peek().kind {
            let p_str = p.clone();
            let compound_ops = [
                "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "|=", "^=",
            ];
            if compound_ops.contains(&p_str.as_str()) {
                self.advance();
                let bin_op = p_str.strip_suffix('=').unwrap().to_owned();
                let val = self.parse_assignment();
                return match expr {
                    Expr::Ident(name) => {
                        let rhs = Expr::Binary {
                            op: bin_op,
                            left: Box::new(Expr::Ident(name.clone())),
                            right: Box::new(val),
                        };
                        Expr::Assign {
                            name,
                            value: Box::new(rhs),
                        }
                    }
                    Expr::Member { obj, prop } => {
                        let rhs = Expr::Binary {
                            op: bin_op,
                            left: Box::new(Expr::Member {
                                obj: obj.clone(),
                                prop: prop.clone(),
                            }),
                            right: Box::new(val),
                        };
                        Expr::MemberAssign {
                            obj,
                            prop,
                            value: Box::new(rhs),
                        }
                    }
                    Expr::Index { obj, index } => {
                        let rhs = Expr::Binary {
                            op: bin_op,
                            left: Box::new(Expr::Index {
                                obj: obj.clone(),
                                index: index.clone(),
                            }),
                            right: Box::new(val),
                        };
                        Expr::IndexAssign {
                            obj,
                            index,
                            value: Box::new(rhs),
                        }
                    }
                    other => other,
                };
            }
        }
        expr
    }

    fn parse_conditional(&mut self) -> Expr {
        let cond = self.parse_nullish_or();
        if self.match_punct("?") {
            let then_expr = self.parse_assignment();
            let _ = self.expect_punct(":");
            let else_expr = self.parse_assignment();
            return Expr::Conditional {
                cond: Box::new(cond),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            };
        }
        cond
    }

    fn parse_nullish_or(&mut self) -> Expr {
        let mut left = self.parse_logical_or();
        while self.match_punct("??") {
            let right = self.parse_logical_or();
            left = Expr::Binary {
                op: "??".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_logical_or(&mut self) -> Expr {
        let mut left = self.parse_logical_and();
        while self.match_punct("||") {
            let right = self.parse_logical_and();
            left = Expr::Binary {
                op: "||".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_logical_and(&mut self) -> Expr {
        let mut left = self.parse_bitwise_or();
        while self.match_punct("&&") {
            let right = self.parse_bitwise_or();
            left = Expr::Binary {
                op: "&&".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_bitwise_or(&mut self) -> Expr {
        let mut left = self.parse_bitwise_xor();
        while self.match_punct("|") {
            let right = self.parse_bitwise_xor();
            left = Expr::Binary {
                op: "|".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_bitwise_xor(&mut self) -> Expr {
        let mut left = self.parse_bitwise_and();
        while self.match_punct("^") {
            let right = self.parse_bitwise_and();
            left = Expr::Binary {
                op: "^".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_bitwise_and(&mut self) -> Expr {
        let mut left = self.parse_equality();
        while self.match_punct("&") {
            let right = self.parse_equality();
            left = Expr::Binary {
                op: "&".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_equality(&mut self) -> Expr {
        let mut left = self.parse_relational();
        while let TokenKind::Punct(p) = &self.peek().kind {
            let op = p.clone();
            if op == "===" || op == "!==" || op == "==" || op == "!=" {
                self.advance();
                let right = self.parse_relational();
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        left
    }

    fn parse_relational(&mut self) -> Expr {
        let mut left = self.parse_shift();
        loop {
            let is_rel = match &self.peek().kind {
                TokenKind::Punct(p) if p == "<" || p == "<=" || p == ">" || p == ">=" => true,
                TokenKind::Keyword(k) if k == "instanceof" || k == "in" => true,
                _ => false,
            };
            if !is_rel {
                break;
            }
            let op = match self.advance().kind {
                TokenKind::Punct(p) => p,
                TokenKind::Keyword(k) => k,
                _ => unreachable!(),
            };
            let right = self.parse_shift();
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_shift(&mut self) -> Expr {
        let mut left = self.parse_additive();
        while let TokenKind::Punct(p) = &self.peek().kind {
            let op = p.clone();
            if op == "<<" || op == ">>" || op == ">>>" {
                self.advance();
                let right = self.parse_additive();
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        left
    }

    fn parse_additive(&mut self) -> Expr {
        let mut left = self.parse_multiplicative();
        while let TokenKind::Punct(p) = &self.peek().kind {
            let op = p.clone();
            if op == "+" || op == "-" {
                self.advance();
                let right = self.parse_multiplicative();
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        left
    }

    fn parse_multiplicative(&mut self) -> Expr {
        let mut left = self.parse_exponentiation();
        while let TokenKind::Punct(p) = &self.peek().kind {
            let op = p.clone();
            if op == "*" || op == "/" || op == "%" {
                self.advance();
                let right = self.parse_exponentiation();
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        left
    }

    fn parse_exponentiation(&mut self) -> Expr {
        let left = self.parse_unary();
        if self.match_punct("**") {
            let right = self.parse_exponentiation();
            Expr::Binary {
                op: "**".to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            }
        } else {
            left
        }
    }

    fn parse_unary(&mut self) -> Expr {
        // `await` 仅在 async 语境是 AwaitExpression 运算符；**非 async 语境
        // 是普通标识符**（`var await = 5; console.log(await)`——此前无条件
        // 消耗 token 致语法错误）
        if self.in_async && self.check_keyword("await") {
            self.advance();
            // async 函数体内 await 为保留字（AwaitExpression 需要操作数）：
            // `void await;`/`await;` 等缺操作数形态 → SyntaxError（非 async
            // 上下文中 await 是普通标识符，不受影响）
            if self.in_async {
                let cant_start = match &self.peek().kind {
                    TokenKind::Punct(p) => {
                        // `:` 覆盖标签形态 `await: ;`（await 后接冒号只能是
                        // 标签，AwaitExpression 不允许）
                        matches!(p.as_str(), ";" | ")" | "]" | "}" | "," | "=" | ":")
                    }
                    TokenKind::Eof => true,
                    _ => false,
                };
                if cant_start {
                    self.record_error("SyntaxError: async 函数中 await 缺少操作数".to_owned());
                }
            }
            let sub = self.parse_unary();
            return Expr::Await(Box::new(sub));
        }
        if let TokenKind::Keyword(kw) = &self.peek().kind {
            let op = kw.clone();
            if op == "delete" || op == "typeof" || op == "void" {
                self.advance();
                let sub = self.parse_unary();
                return Expr::Unary {
                    op,
                    expr: Box::new(sub),
                };
            }
        }
        if let TokenKind::Punct(p) = &self.peek().kind {
            let op = p.clone();
            if op == "++" || op == "--" {
                self.advance();
                let sub = self.parse_unary();
                // `++;` 等前缀缺操作数形态：primary 兜底臂吞掉意外 token
                // 返回 Undefined——此处补记 SyntaxError（受限产生式负例
                // `x\n++;` 依赖该报错；容错解析其余行为不受影响）
                if matches!(sub, Expr::Undefined) {
                    self.record_error("SyntaxError: ++/-- 缺少操作数".to_owned());
                }
                return Expr::Update {
                    op,
                    target: Box::new(sub),
                    prefix: true,
                };
            }
            if op == "-" || op == "+" || op == "!" || op == "~" {
                self.advance();
                let sub = self.parse_unary();
                return Expr::Unary {
                    op,
                    expr: Box::new(sub),
                };
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_expr_primary();
        loop {
            // 标记模板字面量: tag`...${expr}...`
            if let TokenKind::TemplateLiteral {
                quasis,
                raw_quasis,
                raw_exprs,
            } = self.peek().kind.clone()
            {
                self.advance();
                let mut exprs = Vec::with_capacity(raw_exprs.len());
                for raw in raw_exprs {
                    let mut sub_parser = Parser::new(&raw);
                    let sub_expr = sub_parser.parse_expr();
                    exprs.push(sub_expr);
                }
                expr = Expr::TaggedTemplate {
                    tag: Box::new(expr),
                    quasis,
                    raws: raw_quasis,
                    exprs,
                };
                continue;
            }
            // 普通成员访问: obj.prop（prop 可为标识符或关键字，如 `m.default`；
            // 亦可为私有名 `this.#p`——以 `#名` 作属性名）
            if self.match_punct(".") {
                let prop = if self.check_punct("#") {
                    self.advance();
                    match self.peek().kind.clone() {
                        TokenKind::Ident(id) => {
                            self.advance();
                            format!("#{id}")
                        }
                        _ => String::new(),
                    }
                } else {
                    match self.advance().kind {
                        TokenKind::Ident(p) | TokenKind::Keyword(p) => p,
                        _ => String::new(),
                    }
                };
                if !prop.is_empty() {
                    // 查看后续是否为方法调用
                    if self.match_punct("(") {
                        let args = self.parse_args();
                        expr = Expr::MethodCall {
                            receiver: Box::new(expr),
                            method: prop,
                            args,
                        };
                    } else {
                        expr = Expr::Member {
                            obj: Box::new(expr),
                            prop,
                        };
                    }
                }
                continue;
            }
            // 可选链访问: obj?.prop, obj?.[idx], callee?.(args)
            if self.match_punct("?.") {
                if self.match_punct("(") {
                    let args = self.parse_args();
                    expr = Expr::OptionalCall {
                        callee: Box::new(expr),
                        args,
                    };
                } else if self.match_punct("[") {
                    let idx = self.parse_expr();
                    let _ = self.expect_punct("]");
                    expr = Expr::OptionalIndex {
                        obj: Box::new(expr),
                        index: Box::new(idx),
                    };
                } else if let TokenKind::Ident(prop) = self.advance().kind {
                    expr = Expr::OptionalMember {
                        obj: Box::new(expr),
                        prop,
                    };
                }
                continue;
            }
            // 下标访问: obj[idx]
            if self.match_punct("[") {
                let index = self.parse_expr();
                let _ = self.expect_punct("]");
                expr = Expr::Index {
                    obj: Box::new(expr),
                    index: Box::new(index),
                };
                continue;
            }
            // TS 泛型调用实参：`f<A>(x)` / `obj.m<T>(x)`（仅 TS 语境——
            // JS 里 `a < b` 是小于号，且 `(a<b)>(c)` 是合法表达式）
            if self.ts && self.check_punct("<") && self.try_skip_type_args() {
                continue;
            }
            // 普通函数调用: fn(a, b)
            if self.match_punct("(") {
                let args = self.parse_args();
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                };
                continue;
            }
            // TypeScript `as Type` / `as const` 断言零成本剥离
            if self.match_keyword("as") {
                if self.check_keyword("const") {
                    self.advance();
                } else {
                    self.skip_type();
                }
                continue;
            }
            // TS 非空断言 `x!`（仅 TS 语境：JS 里换行后的 `!y` 由 ASI 起新
            // 语句，吞掉会破坏语义）
            if self.ts && self.check_punct("!") {
                self.advance();
                continue;
            }
            break;
        }

        // 后缀自增自减：i++ 或 i--
        // 受限产生式：后缀 ++/-- 前不得有行终止符——换行后的 ++/-- 不
        // 作为后缀继续解析（表达式在此结束，ASI 于 eat_semi 生效；++
        // 留给下一语句作前缀，`var z = 1\n++z` 合法；而 `x\n++;` 因
        // `++` 无操作数由后续解析自然报 SyntaxError——S7.9_A5.x 族）
        let post_op = match &self.peek().kind {
            TokenKind::Punct(p) if p == "++" || p == "--" => Some(p.clone()),
            _ => None,
        };
        if let Some(op) = post_op {
            if self.nl_before_current() {
                return expr;
            }
            // 后缀目标为字面量（`;-->` 解析出的 `undefined--` 等）：
            // Invalid left-hand side → SyntaxError
            if matches!(
                expr,
                Expr::Undefined
                    | Expr::Boolean(_)
                    | Expr::Number(_)
                    | Expr::BigInt(_)
                    | Expr::String(_)
                    | Expr::Null
            ) {
                self.record_error("SyntaxError: 后缀 ++/-- 目标不能为字面量".to_owned());
            }
            self.advance();
            return Expr::Update {
                op,
                target: Box::new(expr),
                prefix: false,
            };
        }

        expr
    }

    fn parse_args(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
            if self.match_punct("...") {
                let sub = self.parse_expr();
                args.push(Expr::Spread(Box::new(sub)));
            } else {
                args.push(self.parse_expr());
            }
            if !self.match_punct(",") {
                break;
            }
        }
        let _ = self.expect_punct(")");
        args
    }

    fn parse_expr_primary(&mut self) -> Expr {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Number(n) => {
                self.advance();
                Expr::Number(n)
            }
            TokenKind::BigInt(b) => {
                self.advance();
                Expr::BigInt(b)
            }
            TokenKind::String(s) => {
                self.advance();
                Expr::String(s)
            }
            TokenKind::RegexLiteral { pattern, flags } => {
                self.advance();
                Expr::RegExp { pattern, flags }
            }
            TokenKind::TemplateLiteral {
                quasis, raw_exprs, ..
            } => {
                self.advance();
                let mut exprs = Vec::with_capacity(raw_exprs.len());
                for raw in raw_exprs {
                    let mut sub_parser = Parser::new(&raw);
                    let expr = sub_parser.parse_expr();
                    exprs.push(expr);
                }
                Expr::TemplateLiteral { quasis, exprs }
            }
            TokenKind::Keyword(kw) => {
                self.advance();
                match kw.as_str() {
                    "true" => Expr::Boolean(true),
                    "false" => Expr::Boolean(false),
                    "null" => Expr::Null,
                    "undefined" => Expr::Undefined,
                    "this" => Expr::This,
                    "super" => {
                        if self.super_disallowed {
                            self.record_error("SyntaxError: 'super' 关键字在此不可用".to_owned());
                        }
                        Expr::Super
                    }
                    // 纯语法关键字不得作标识符兜底（悬空 `else {}` 曾被
                    // 兜成 Ident 表达式静默接受——Node 22 报
                    // "Unexpected token 'else'"）
                    "else" | "in" | "instanceof" | "typeof" | "void" | "delete" | "case"
                    | "catch" | "finally" | "do" | "default" | "extends" | "with" | "enum" => {
                        self.record_error(format!("SyntaxError: 意外的关键字 '{kw}'"));
                        Expr::Ident(kw)
                    }
                    // 类表达式：`return class {...}` / `x = class extends B {}`
                    "class" => self.parse_class_expr(),
                    "function" => {
                        let is_generator = self.match_punct("*");
                        let mut def = self.parse_function_def(false, is_generator);
                        def.is_generator = is_generator;
                        Expr::Function(def)
                    }
                    "async" => {
                        // async 函数表达式：async 与 function **同行**才构
                        // 成修饰符（换行后是 ASI 两语句——语句级同规）
                        if !self.nl_before_current() && self.match_keyword("function") {
                            let is_generator = self.match_punct("*");
                            let mut def = self.parse_function_def(true, is_generator);
                            def.is_async = true;
                            def.is_generator = is_generator;
                            Expr::Function(def)
                        } else if self.is_arrow_function() {
                            self.parse_arrow_function_from_paren(true)
                        } else if self.ts && self.check_punct("<") && {
                            // TS 泛型 async 箭头：`async <T>(v: T) => v`
                            let save = self.pos;
                            let ok = self.skip_angle_group() && self.check_punct("(");
                            self.pos = save;
                            ok
                        } {
                            let _ = self.skip_angle_group();
                            if self.is_arrow_function() {
                                self.parse_arrow_function_from_paren(true)
                            } else {
                                Expr::Ident(kw)
                            }
                        } else if let TokenKind::Ident(id) = self.peek().kind.clone() {
                            if self.peek_ahead(1).kind == TokenKind::Punct("=>".to_owned()) {
                                self.advance(); // 消耗 id
                                self.advance(); // 消耗 =>
                                // 单参 async 箭头（`async str => await x`）：
                                // 体须在 async 语境下解析（否则 await 被当
                                // 标识符 → 语法错误；axios 的 encodeText 形态）
                                let outer_async = self.in_async;
                                self.in_async = true;
                                let body = self.parse_arrow_body();
                                self.in_async = outer_async;
                                Expr::Function(FunctionDef {
                                    name: String::new(),
                                    params: vec![id],
                                    is_var_args: false,
                                    body,
                                    is_async: true,
                                    is_generator: false,
                                    is_arrow: true,
                                })
                            } else {
                                Expr::Ident(kw)
                            }
                        } else {
                            Expr::Ident(kw)
                        }
                    }
                    "new" => {
                        // `new.target` 元属性（被 new 调用时为新目标构造器）——
                        // 须在 callee 解析前拦截，否则 `.target` 会被当作成员访问
                        // （zod ZodError.cjs：`const actualProto = new.target.prototype`）。
                        // 表示为保留标识符，由编译器分配槽位、VM 在构造入口写入。
                        if self.check_punct(".") {
                            let save = self.pos;
                            self.advance(); // '.'
                            if let TokenKind::Ident(p) | TokenKind::Keyword(p) =
                                self.peek().kind.clone()
                            {
                                if p == "target" {
                                    self.advance();
                                    return Expr::Ident(crate::ast::NEW_TARGET_SYM.to_owned());
                                }
                            }
                            self.pos = save;
                        }
                        // callee 支持成员访问链（`new a.B()` / `new ns.Foo.Ctor()`）：
                        // 只吃 `.` 属性访问，`(` 归 New 的实参列表
                        let mut callee = self.parse_expr_primary();
                        loop {
                            if self.match_punct(".") {
                                if let TokenKind::Ident(prop) | TokenKind::Keyword(prop) =
                                    self.advance().kind
                                {
                                    callee = Expr::Member {
                                        obj: Box::new(callee),
                                        prop,
                                    };
                                    continue;
                                }
                            }
                            break;
                        }
                        // `new Map<string, ResourceService<Entity>>()`
                        if self.ts && self.check_punct("<") {
                            let _ = self.try_skip_type_args();
                        }
                        let args = if self.match_punct("(") {
                            self.parse_args()
                        } else {
                            Vec::new()
                        };
                        Expr::New {
                            callee: Box::new(callee),
                            args,
                        }
                    }
                    // 动态导入 `import(specifier[, options])`：编译为专管
                    // 全局调用 __aluka_dynamic_import__（VM 侧保证「恒返回
                    // Promise」的动态导入语义）
                    "import" if self.peek().kind == TokenKind::Punct("(".to_owned()) => {
                        self.advance(); // 消耗 `(`
                        let mut args = Vec::new();
                        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
                            args.push(self.parse_expr());
                            if !self.match_punct(",") {
                                break;
                            }
                        }
                        let _ = self.expect_punct(")");
                        Expr::Call {
                            callee: Box::new(Expr::Ident("__aluka_dynamic_import__".to_owned())),
                            args,
                        }
                    }
                    // `import.meta`：元属性 → 预注册的全局名 __importMeta
                    // （关键字自身已在 match 前消耗）
                    "import" if self.peek().kind == TokenKind::Punct(".".to_owned()) => {
                        self.advance(); // 消耗 '.'
                        let prop = match self.advance().kind {
                            TokenKind::Ident(p) | TokenKind::Keyword(p) => p,
                            _ => "meta".to_owned(),
                        };
                        if prop == "meta" {
                            Expr::Ident("__importMeta".to_owned())
                        } else {
                            Expr::Ident("import".to_owned())
                        }
                    }
                    _ => Expr::Ident(kw),
                }
            }
            TokenKind::Ident(id) => {
                if self.peek_ahead(1).kind == TokenKind::Punct("=>".to_owned()) {
                    self.advance(); // 消耗 id
                    self.advance(); // 消耗 =>
                    let body = self.parse_arrow_body();
                    Expr::Function(FunctionDef {
                        name: String::new(),
                        params: vec![id],
                        is_var_args: false,
                        body,
                        is_async: false,
                        is_generator: false,
                        is_arrow: true,
                    })
                } else {
                    self.advance();
                    Expr::Ident(id)
                }
            }
            TokenKind::Punct(p) if p == "(" => {
                if self.is_arrow_function() {
                    self.parse_arrow_function_from_paren(false)
                } else {
                    self.advance();
                    // 逗号序列：`(a, b, c)` 逐项求值取末项（单表达式退化为
                    // 原形态；`(0, eval)` 间接调用惯用法依赖此形态）
                    let mut exprs = vec![self.parse_expr()];
                    while self.match_punct(",") {
                        if self.check_punct(")") {
                            break;
                        }
                        exprs.push(self.parse_expr());
                    }
                    let _ = self.expect_punct(")");
                    if exprs.len() == 1 {
                        exprs.pop().unwrap()
                    } else {
                        Expr::Seq(exprs)
                    }
                }
            }
            TokenKind::Punct(p) if p == "[" => {
                self.advance();
                let mut elements = Vec::new();
                loop {
                    if self.check_punct("]") || self.peek().kind == TokenKind::Eof {
                        break;
                    }
                    // 数组 elision（`[,,,1,2]` / `[1,2,,4,5]`）：逗号直接
                    // 产生一个空洞元素；尾随逗号（`[1,]`）在解析元素后由
                    // 下方 `]` 判定终止，不产生空洞
                    if self.match_punct(",") {
                        elements.push(Expr::Undefined);
                        continue;
                    }
                    if self.match_punct("...") {
                        let sub = self.parse_expr();
                        elements.push(Expr::Spread(Box::new(sub)));
                    } else {
                        elements.push(self.parse_expr());
                    }
                    if !self.match_punct(",") {
                        break;
                    }
                }
                let _ = self.expect_punct("]");
                Expr::Array(elements)
            }
            TokenKind::Punct(p) if p == "{" => {
                self.advance();
                let mut props = Vec::new();
                let mut proto_seen = false;
                // 对象字面量方法有 HomeObject：super 合法——清除禁用旗标
                let outer_super = self.super_disallowed;
                self.super_disallowed = false;
                while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                    // 0. 检查是否为对象展开属性: ...expr
                    if self.match_punct("...") {
                        let inner = self.parse_expr();
                        props.push(ObjectProp {
                            key: PropKey::Literal(String::new()),
                            value: PropValue::Spread(inner),
                        });
                        if !self.match_punct(",") {
                            break;
                        }
                        continue;
                    }

                    // 1. 检查是否为 getter: get prop() { ... }
                    //    `get`/`set` 后跟 `(` 时是名为 get/set 的方法简写
                    //    （如 Proxy handler 的 `{ get(t,k,r) {} }`），走普通方法路径
                    if let TokenKind::Ident(ref id) = self.peek().kind {
                        if id == "get"
                            && !self.peek_ahead(1).is_punct(":")
                            && !self.peek_ahead(1).is_punct(",")
                            && !self.peek_ahead(1).is_punct("}")
                            && !self.peek_ahead(1).is_punct("(")
                        {
                            self.advance(); // 消耗 "get"
                            let key = self.parse_prop_key();
                            let _ = self.expect_punct("(");
                            let _ = self.expect_punct(")");
                            let body_stmt = self.parse_stmt();
                            let body = match body_stmt {
                                SpannedStmt {
                                    stmt: Stmt::Block(stmts),
                                    ..
                                } => stmts,
                                other => vec![other],
                            };
                            props.push(ObjectProp {
                                key,
                                value: PropValue::Getter(FunctionDef {
                                    name: String::new(),
                                    params: Vec::new(),
                                    is_var_args: false,
                                    body,
                                    is_async: false,
                                    is_generator: false,
                                    is_arrow: false,
                                }),
                            });
                            if !self.match_punct(",") {
                                break;
                            }
                            continue;
                        } else if id == "set"
                            && !self.peek_ahead(1).is_punct(":")
                            && !self.peek_ahead(1).is_punct(",")
                            && !self.peek_ahead(1).is_punct("}")
                            && !self.peek_ahead(1).is_punct("(")
                        {
                            self.advance(); // 消耗 "set"
                            let key = self.parse_prop_key();
                            let _ = self.expect_punct("(");
                            let param = if let TokenKind::Ident(param_name) = self.advance().kind {
                                // strict 语义：setter 形参名不得为 arguments/eval
                                if self.strict
                                    && matches!(param_name.as_str(), "arguments" | "eval")
                                {
                                    self.record_error(format!(
                                        "SyntaxError: 形参名不允许为 {param_name}"
                                    ));
                                }
                                param_name
                            } else {
                                String::new()
                            };
                            let _ = self.expect_punct(")");
                            let body_stmt = self.parse_stmt();
                            let body = match body_stmt {
                                SpannedStmt {
                                    stmt: Stmt::Block(stmts),
                                    ..
                                } => stmts,
                                other => vec![other],
                            };
                            props.push(ObjectProp {
                                key,
                                value: PropValue::Setter(FunctionDef {
                                    name: String::new(),
                                    params: if param.is_empty() {
                                        Vec::new()
                                    } else {
                                        vec![param]
                                    },
                                    is_var_args: false,
                                    body,
                                    is_async: false,
                                    is_generator: false,
                                    is_arrow: false,
                                }),
                            });
                            if !self.match_punct(",") {
                                break;
                            }
                            continue;
                        }
                    }

                    // 2. 普通属性、计算属性或方法简写
                    // async 方法修饰符：`async foo() {}` / `async *foo() {}`
                    // （async 后随方法名而非 : , } ( => 才构成修饰符）；
                    // 生成器前缀 `*foo() {}` 同点支持
                    let mut m_is_async = false;
                    let mut m_is_generator = false;
                    if self.check_keyword("async")
                        && !self.peek_ahead(1).is_punct(":")
                        && !self.peek_ahead(1).is_punct(",")
                        && !self.peek_ahead(1).is_punct("}")
                        && !self.peek_ahead(1).is_punct("(")
                        && !self.peek_ahead(1).is_punct("=>")
                    {
                        self.advance();
                        m_is_async = true;
                        m_is_generator = self.match_punct("*");
                    } else if self.check_punct("*")
                        && !self.peek_ahead(1).is_punct("(")
                        && !self.peek_ahead(1).is_punct(":")
                    {
                        self.advance();
                        m_is_generator = true;
                    }
                    let key = self.parse_prop_key();
                    if self.match_punct("(") {
                        let mut params = Vec::new();
                        let mut is_var_args = false;
                        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
                            if self.match_punct("...") {
                                is_var_args = true;
                            }
                            if let Some(param_name) = self.advance_ident_like() {
                                params.push(param_name);
                                self.skip_type_annotation();
                            }
                            if !self.match_punct(",") {
                                break;
                            }
                        }
                        let _ = self.expect_punct(")");
                        self.skip_type_annotation();
                        // 方法体的生成器/async 语境：`{ *gen() { yield 1 } }`
                        // 体内 yield 须被识别（此前未置 in_generator 致
                        // `yield 2` 解析失败——axios 的 ReadableStream
                        // 源对象大量使用 async/生成器方法）
                        let outer_gen = self.in_generator;
                        let outer_async = self.in_async;
                        self.in_generator = m_is_generator;
                        self.in_async = m_is_async;
                        let body_stmt = self.parse_stmt();
                        self.in_generator = outer_gen;
                        self.in_async = outer_async;
                        let body = match body_stmt {
                            SpannedStmt {
                                stmt: Stmt::Block(stmts),
                                ..
                            } => stmts,
                            other => vec![other],
                        };
                        let m_name = match &key {
                            PropKey::Literal(n) => n.clone(),
                            _ => String::new(),
                        };
                        props.push(ObjectProp {
                            key,
                            value: PropValue::Expr(Expr::Function(FunctionDef {
                                name: m_name,
                                params,
                                is_var_args,
                                body,
                                is_async: m_is_async,
                                is_generator: m_is_generator,
                                is_arrow: false,
                            })),
                        });
                    } else if self.check_punct(",") || self.check_punct("}") {
                        // 属性简写：`{ port }` ≡ `{ port: port }`
                        let n = match &key {
                            PropKey::Literal(n) => n.clone(),
                            _ => String::new(),
                        };
                        props.push(ObjectProp {
                            key,
                            value: PropValue::Expr(Expr::Ident(n)),
                        });
                    } else {
                        // 冒号形态的重复 __proto__（含字符串键 `'__proto__':`）
                        // → SyntaxError（Node 22: Duplicate __proto__ fields）
                        if matches!(&key, PropKey::Literal(n) if n == "__proto__") {
                            if proto_seen {
                                self.record_error(
                                    "SyntaxError: 对象字面量不允许重复的 __proto__ 属性".to_owned(),
                                );
                            }
                            proto_seen = true;
                        }
                        let _ = self.expect_punct(":");
                        let val = self.parse_expr();
                        props.push(ObjectProp {
                            key,
                            value: PropValue::Expr(val),
                        });
                    }
                    if !self.match_punct(",") {
                        break;
                    }
                }
                let _ = self.expect_punct("}");
                self.super_disallowed = outer_super;
                Expr::Object(props)
            }
            TokenKind::Punct(ref p) if p == "/" || p == "/=" => {
                if let Some(regex) = self.parse_regexp_literal() {
                    regex
                } else {
                    // 主位的 `/` 只能是正则字面量起点；解析失败（含行终结符
                    // 等非法形态）即 SyntaxError——兜底静默会让
                    // `/a<LF>/` 负例被接受
                    self.record_error("SyntaxError: 非法的正则字面量".to_owned());
                    self.advance();
                    Expr::Undefined
                }
            }
            _ => {
                // 表达式主位遇到词法错误 token（未终止字符串、非法 BigInt
                // 分隔符等）：判死——语句首已有判死通道，此处补表达式中间
                // 形态（`0b0_n;` 等曾被兜底臂静默吞掉）
                if let TokenKind::LexError(msg) = &self.peek().kind {
                    let msg = msg.clone();
                    self.record_error(format!("SyntaxError: {msg}"));
                    self.advance();
                    return Expr::Undefined;
                }
                // TS 泛型箭头函数：`const id = <T>(x: T): T => x`。
                // 表达式**主位**的 `<` 不可能是小于号（无左操作数），故无歧义；
                // 仅在尖括号后可接箭头形参表时按此解释，否则维持判死通道。
                if self.ts && self.check_punct("<") && {
                    let save = self.pos;
                    let ok = self.skip_angle_group() && self.check_punct("(");
                    self.pos = save;
                    ok
                } {
                    let _ = self.skip_angle_group();
                    if self.is_arrow_function() {
                        return self.parse_arrow_function_from_paren(false);
                    }
                }
                // 比较类标点永不处于表达式主位（`;-->` 的 `>` 曾被静默吞）
                if let TokenKind::Punct(p) = &self.peek().kind {
                    if matches!(
                        p.as_str(),
                        ">" | "<" | ">=" | "<=" | "===" | "!==" | "==" | "!="
                    ) {
                        self.record_error(format!("SyntaxError: 意外的标记 '{p}'"));
                    }
                }
                self.advance();
                Expr::Undefined
            }
        }
    }

    /// 解析正则表达式字面量 `/pattern/flags` 并重同步 Token 游标
    fn parse_regexp_literal(&mut self) -> Option<Expr> {
        let tok = self.peek();
        let start = tok.start;
        let bytes = self.src.as_bytes();
        if start >= bytes.len() || bytes[start] != b'/' {
            return None;
        }
        let mut idx = start + 1;
        let mut in_class = false;
        let mut closed = false;
        while idx < bytes.len() {
            let b = bytes[idx];
            if b == b'\n' || b == b'\r' {
                break;
            }
            // U+2028/U+2029（LS/PS，UTF-8: E2 80 A8/A9）同为行终结符——
            // 正则字面量禁止（invalid-regexp-ls/ps 负例）
            if b == 0xE2
                && bytes.get(idx + 1) == Some(&0x80)
                && matches!(bytes.get(idx + 2), Some(0xA8) | Some(0xA9))
            {
                break;
            }
            if b == b'\\' {
                idx += 1;
                if idx < bytes.len() {
                    idx += 1;
                }
                continue;
            }
            if b == b'[' {
                in_class = true;
            } else if b == b']' {
                in_class = false;
            } else if b == b'/' && !in_class {
                closed = true;
                break;
            }
            idx += 1;
        }

        if !closed {
            return None;
        }

        let pattern = self.src[start + 1..idx].to_owned();
        idx += 1; // 消耗闭合 '/'

        let flags_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
            idx += 1;
        }
        let flags = self.src[flags_start..idx].to_owned();

        // 推进 tokens 游标至 idx 之后
        while self.pos < self.tokens.len() && self.tokens[self.pos].start < idx {
            self.pos += 1;
        }

        Some(Expr::RegExp { pattern, flags })
    }

    /// 逗号序列表达式：`a, b, c`（for 子句语境；单个时透传）。
    fn parse_expr_sequence(&mut self) -> Expr {
        let first = self.parse_expr();
        if !self.check_punct(",") {
            return first;
        }
        let mut exprs = vec![first];
        while self.match_punct(",") {
            exprs.push(self.parse_expr());
        }
        Expr::Sequence { exprs }
    }

    fn parse_prop_key(&mut self) -> PropKey {
        if self.match_punct("[") {
            let expr = self.parse_expr();
            let _ = self.expect_punct("]");
            PropKey::Computed(expr)
        } else if let TokenKind::Number(_k) = self.peek().kind.clone() {
            // 数字键：`{ 0: target }`（真实包常见；键取字面文本）
            self.advance();
            let text = self.tokens[self.pos - 1].text.clone();
            PropKey::Literal(text)
        } else if let TokenKind::Ident(k) = self.peek().kind.clone() {
            self.advance();
            PropKey::Literal(k)
        } else if let TokenKind::String(k) = self.peek().kind.clone() {
            self.advance();
            PropKey::Literal(k)
        } else if let TokenKind::Keyword(k) = self.peek().kind.clone() {
            self.advance();
            PropKey::Literal(k)
        } else {
            PropKey::Literal(String::new())
        }
    }

    fn is_arrow_function(&self) -> bool {
        if !self.check_punct("(") {
            return false;
        }
        let mut depth = 0;
        let mut i = self.pos;
        while i < self.tokens.len() {
            let tok = &self.tokens[i];
            if let TokenKind::Punct(p) = &tok.kind {
                if p == "(" {
                    depth += 1;
                } else if p == ")" {
                    depth -= 1;
                    if depth == 0 {
                        // 查看 ) 之后是否有 =>
                        let mut j = i + 1;
                        // 跳过类型注解 (e.g. `): void =>`)
                        if j < self.tokens.len()
                            && self.tokens[j].kind == TokenKind::Punct(":".to_owned())
                        {
                            j += 1;
                            // 类型注解扫描：括号/方括号/花括号内视为类型内容
                            // （`string[]` / `Array<T>` / `{a: number}`），
                            // 深度归零后的结构性 token 才是边界（`?`/`:`/`,`/
                            // `)`/`]`/`}`/`;`/`=`）或命中箭头 `=>`——否则三元
                            // 表达式 `t ? (1) : async s => ...` 的 `:` 会让
                            // 扫描一路吃到 else 分支的 `=>`，把 then 分支的
                            // 括号误判成箭头形参表
                            let mut type_depth = 0i32;
                            while j < self.tokens.len() {
                                if let TokenKind::Punct(p) = &self.tokens[j].kind {
                                    match p.as_str() {
                                        "(" | "[" | "{" | "<" => type_depth += 1,
                                        ")" | "]" | "}" => {
                                            if type_depth == 0 {
                                                break;
                                            }
                                            type_depth -= 1;
                                        }
                                        // `>>`/`>>>` 是单 token：一次收口多层泛型
                                        _ if p.starts_with('>') && p.chars().all(|c| c == '>') => {
                                            type_depth -= p.len() as i32;
                                        }
                                        "=>" if type_depth == 0 => break,
                                        ";" | "?" | ":" | "," | "=" if type_depth == 0 => break,
                                        _ => {}
                                    }
                                }
                                j += 1;
                            }
                        }
                        if j < self.tokens.len() {
                            return self.tokens[j].kind == TokenKind::Punct("=>".to_owned());
                        }
                        return false;
                    }
                }
            }
            i += 1;
        }
        false
    }

    fn parse_arrow_body(&mut self) -> Vec<SpannedStmt> {
        if self.check_punct("{") {
            let stmt = self.parse_stmt();
            match stmt {
                SpannedStmt {
                    stmt: Stmt::Block(stmts),
                    ..
                } => stmts,
                other => vec![other],
            }
        } else {
            let expr = self.parse_assignment();
            let line = self.cur_line();
            vec![Self::at(line, Stmt::Return(Some(expr)))]
        }
    }

    fn parse_arrow_function_from_paren(&mut self, is_async: bool) -> Expr {
        self.advance(); // 消耗 (
        let mut params = Vec::new();
        let mut is_var_args = false;
        let mut prologue_stmts: Vec<SpannedStmt> = Vec::new();
        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
            if self.match_punct("...") {
                is_var_args = true;
            }
            if self.check_punct("[") || self.check_punct("{") {
                // 解构参数：`([a,b]) => {}` / `({a}) => {}`——占位名 +
                // DestructureDecl prologue（同具名函数路径）
                let pattern = self.parse_var_pattern();
                let param_name = format!("__param_{}__", params.len());
                params.push(param_name.clone());
                let dline = self.cur_line();
                // 解构形参默认值（`({allOwnKeys} = {}) => {}`——axios 的
                // extend 工具函数即此形态）
                let init = if self.match_punct("=") {
                    let def = self.parse_expr();
                    Expr::Conditional {
                        cond: Box::new(Expr::Binary {
                            op: "===".to_owned(),
                            left: Box::new(Expr::Ident(param_name.clone())),
                            right: Box::new(Expr::Undefined),
                        }),
                        then_expr: Box::new(def),
                        else_expr: Box::new(Expr::Ident(param_name.clone())),
                    }
                } else {
                    Expr::Ident(param_name)
                };
                prologue_stmts.push(Self::at(dline, Stmt::DestructureDecl { pattern, init }));
                self.skip_param_suffix();
            } else if let Some(p_name) = self.advance_ident_like() {
                params.push(p_name.clone());
                self.skip_param_suffix();
                // 默认参数 `param = default`：与具名函数同款
                // prologue 条件赋值（undefined 时取默认值）
                if self.match_punct("=") {
                    let default_expr = self.parse_expr();
                    let pline = self.cur_line();
                    prologue_stmts.push(Self::at(
                        pline,
                        Stmt::Expr(Expr::Assign {
                            name: p_name.clone(),
                            value: Box::new(Expr::Conditional {
                                cond: Box::new(Expr::Binary {
                                    op: "===".to_owned(),
                                    left: Box::new(Expr::Ident(p_name.clone())),
                                    right: Box::new(Expr::Undefined),
                                }),
                                then_expr: Box::new(default_expr),
                                else_expr: Box::new(Expr::Ident(p_name)),
                            }),
                        }),
                    ));
                }
            }
            if !self.match_punct(",") {
                break;
            }
        }
        let _ = self.expect_punct(")");
        self.skip_type_annotation();
        let _ = self.expect_punct("=>");
        // 箭头函数体语境：`async () => { await x }` 体内 await 须被识别
        //（此前未置 in_async，await 被当标识符 → 语法错误）
        let outer_async = self.in_async;
        self.in_async = is_async;
        let mut body = self.parse_arrow_body();
        self.in_async = outer_async;
        // 参数 prologue（解构绑定 / 默认值）注入函数体首部。
        //
        // `parse_arrow_body` 对块体**已展平**为语句向量（不保留 Stmt::Block
        // 包装）、表达式体为 `vec![Stmt::Return(expr)]`——此前按
        // `body.last_mut()` 匹配 `Stmt::Block` 的写法两者都不命中，
        // prologue 从未注入，致箭头函数解构参数/默认参数全部失效
        //（`([u,v])=>u+v` → ReferenceError: u is not defined）。
        // 直接前插到向量首部即对两种体型均正确。
        if !prologue_stmts.is_empty() {
            let mut all = prologue_stmts;
            all.append(&mut body);
            body = all;
        }
        Expr::Function(FunctionDef {
            name: String::new(),
            params,
            is_var_args,
            body,
            is_async,
            is_generator: false,
            is_arrow: true,
        })
    }

    fn parse_import_stmt(&mut self) -> Stmt {
        self.advance(); // 消耗 import
        let mut specifiers = Vec::new();

        // 纯副作用导入：import "mod.css";
        if let TokenKind::String(s) = self.peek().kind.clone() {
            self.advance();
            self.eat_semi();
            return Stmt::Import(ImportDecl {
                source: s,
                specifiers,
            });
        }

        // 默认导入：import x from 'mod'; 或 import x, { a } from 'mod';
        if let TokenKind::Ident(id) = self.peek().kind.clone() {
            if id != "from" {
                self.advance();
                specifiers.push(ImportSpecifier::Default(id));
                if self.match_punct(",") {
                    // 可能后接 { ... } 或 * as ns
                }
            }
        }

        // 命名空间导入：import * as ns from 'mod';
        if self.match_punct("*") {
            let _ = self.expect_keyword("as");
            let ns = match self.advance().kind {
                TokenKind::Ident(s) | TokenKind::Keyword(s) => s,
                _ => "ns".to_owned(),
            };
            specifiers.push(ImportSpecifier::Namespace(ns));
        } else if self.match_punct("{") {
            // 命名导入：import { a, b as c } from 'mod';
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                // 内联类型说明符：`import { createApp, type App } from ...`
                // （`type` 后随另一标识符/关键字才是修饰符；`{ type }` /
                // `{ type as t }` 仍是名为 type 的绑定）
                if matches!(&self.peek().kind, TokenKind::Ident(t) if t == "type")
                    && matches!(
                        self.peek_ahead(1).kind,
                        TokenKind::Ident(_) | TokenKind::Keyword(_)
                    )
                    && !matches!(&self.peek_ahead(1).kind, TokenKind::Ident(t) if t == "as")
                {
                    self.advance();
                }
                let imported = match self.advance().kind {
                    TokenKind::Ident(s) | TokenKind::Keyword(s) => s,
                    _ => break,
                };
                let local = if self.match_keyword("as") {
                    match self.advance().kind {
                        TokenKind::Ident(s) | TokenKind::Keyword(s) => s,
                        _ => imported.clone(),
                    }
                } else {
                    imported.clone()
                };
                specifiers.push(ImportSpecifier::Named { local, imported });
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct("}");
        }

        let _ = self.expect_keyword("from");
        let source = match self.advance().kind {
            TokenKind::String(s) => s,
            _ => String::new(),
        };
        self.eat_semi();
        Stmt::Import(ImportDecl { source, specifiers })
    }

    fn parse_export_stmt(&mut self) -> Stmt {
        self.advance(); // 消耗 export

        // `export type X = ...` / `export interface X { ... }`：类型层声明
        // 无运行时导出面，整体剥离（`export type { A }` 亦然）
        if self.skip_ts_declaration() {
            return Stmt::Block(Vec::new());
        }
        if self.check_soft_keyword("type") && self.peek_ahead(1).is_punct("{") {
            self.advance();
            let _ = self.skip_balanced("{", "}");
            let _ = self.match_keyword("from");
            if let TokenKind::String(_) = self.peek().kind {
                self.advance();
            }
            self.eat_semi();
            return Stmt::Block(Vec::new());
        }

        // export default ...
        if self.match_keyword("default") {
            let expr = if self.match_keyword("function") {
                let is_generator = self.match_punct("*");
                let mut def = self.parse_function_def(false, is_generator);
                def.is_generator = is_generator;
                Expr::Function(def)
            } else {
                self.parse_expr()
            };
            self.eat_semi();
            return Stmt::Export(ExportDecl::Default(Box::new(expr)));
        }

        // export * from 'mod'; 或 export * as ns from 'mod';
        if self.match_punct("*") {
            let alias = if self.match_keyword("as") {
                match self.advance().kind {
                    TokenKind::Ident(s) | TokenKind::Keyword(s) => Some(s),
                    _ => None,
                }
            } else {
                None
            };
            let _ = self.expect_keyword("from");
            let source = match self.advance().kind {
                TokenKind::String(s) => s,
                _ => String::new(),
            };
            self.eat_semi();
            return Stmt::Export(ExportDecl::All { source, alias });
        }

        // export { a, b as c } [from 'mod'];
        if self.match_punct("{") {
            let mut specifiers = Vec::new();
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                let local = match self.advance().kind {
                    TokenKind::Ident(s) | TokenKind::Keyword(s) => s,
                    _ => break,
                };
                let exported = if self.match_keyword("as") {
                    match self.advance().kind {
                        TokenKind::Ident(s) | TokenKind::Keyword(s) => s,
                        _ => local.clone(),
                    }
                } else {
                    local.clone()
                };
                specifiers.push(ExportSpecifier { local, exported });
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct("}");
            let source = if self.match_keyword("from") {
                match self.advance().kind {
                    TokenKind::String(s) => Some(s),
                    _ => None,
                }
            } else {
                None
            };
            self.eat_semi();
            return Stmt::Export(ExportDecl::Named {
                decl: None,
                specifiers,
                source,
            });
        }

        // export const/let/var/function/class ...
        let inner = self.parse_stmt();
        Stmt::Export(ExportDecl::Named {
            decl: Some(Box::new(inner)),
            specifiers: Vec::new(),
            source: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_statements_and_expressions() {
        let code = r#"
            let a: number = 10;
            const b = 20;
            if (a < b) {
                a = a + 5;
            } else {
                a = 0;
            }
            return a;
        "#;
        let prog = parse(code);
        assert_eq!(prog.body.len(), 4);
    }

    #[test]
    fn parses_classes_and_optional_chaining_and_try() {
        let code = r#"
            class User extends Person {
                constructor(name: string) {
                    this.name = name;
                }
                greet() {
                    return this.name?.toUpperCase();
                }
            }
            try {
                let u = new User("alice");
                let val = u?.greet();
            } catch (e) {
                return null;
            }
        "#;
        let prog = parse(code);
        assert_eq!(prog.body.len(), 2);
    }

    #[test]
    fn parses_esm_import_and_export_declarations() {
        let code = r#"
            import React, { useState as useMyState } from 'react';
            import * as utils from './utils';
            import 'style.css';

            export const answer = 42;
            export default function run() { return answer; };
            export { a, b as c } from 'other';
            export * as ns from 'bar';
        "#;
        let prog = parse(code);
        assert_eq!(prog.body.len(), 7);
        assert!(matches!(&prog.body[0].stmt, Stmt::Import(..)));
        assert!(matches!(&prog.body[1].stmt, Stmt::Import(..)));
        assert!(matches!(&prog.body[2].stmt, Stmt::Import(..)));
        assert!(matches!(&prog.body[3].stmt, Stmt::Export(..)));
        assert!(matches!(&prog.body[4].stmt, Stmt::Export(..)));
        assert!(matches!(&prog.body[5].stmt, Stmt::Export(..)));
        assert!(matches!(&prog.body[6].stmt, Stmt::Export(..)));
    }

    #[test]
    fn parses_template_literal_and_destructuring_features() {
        let code = r#"
            let msg = `hello ${name}, sum is ${1 + 2}!`;
            const { x, y = 10, z: renamed = 20 } = obj;
            const [a, b = 5, ...rest] = arr;
            function greet({ name = "guest" }, [prefix = "Hi"]) {
                return `${prefix} ${name}`;
            }
        "#;
        let prog = parse(code);
        assert_eq!(prog.body.len(), 4);

        // 1. 模板字符串
        if let SpannedStmt {
            stmt:
                Stmt::VarDecl {
                    init: Some(Expr::TemplateLiteral { quasis, exprs }),
                    ..
                },
            ..
        } = &prog.body[0]
        {
            assert_eq!(quasis.len(), 3);
            assert_eq!(quasis[0], "hello ");
            assert_eq!(quasis[1], ", sum is ");
            assert_eq!(quasis[2], "!");
            assert_eq!(exprs.len(), 2);
        } else {
            panic!("期望解析出 TemplateLiteral");
        }

        // 2. 对象解构与默认值
        if let SpannedStmt {
            stmt:
                Stmt::DestructureDecl {
                    pattern: VarPattern::Object(props),
                    ..
                },
            ..
        } = &prog.body[1]
        {
            assert_eq!(props.len(), 3);
            assert!(props[0].default_value.is_none());
            assert!(props[1].default_value.is_some());
            assert_eq!(props[2].key, "z");
            assert!(props[2].default_value.is_some());
        } else {
            panic!("期望解析出对象解构");
        }

        // 3. 数组解构与默认值
        if let SpannedStmt {
            stmt:
                Stmt::DestructureDecl {
                    pattern: VarPattern::Array(elems),
                    ..
                },
            ..
        } = &prog.body[2]
        {
            assert_eq!(elems.len(), 3);
            assert!(elems[0].default_value.is_none());
            assert!(elems[1].default_value.is_some());
            assert!(elems[2].is_rest);
        } else {
            panic!("期望解析出数组解构");
        }

        // 4. 函数形参解构降级
        if let SpannedStmt {
            stmt: Stmt::Function(func_def),
            ..
        } = &prog.body[3]
        {
            assert_eq!(func_def.params.len(), 2);
            assert_eq!(func_def.params[0], "__param_0__");
            assert_eq!(func_def.params[1], "__param_1__");
            assert!(matches!(
                &func_def.body[0].stmt,
                Stmt::DestructureDecl { .. }
            ));
            assert!(matches!(
                &func_def.body[1].stmt,
                Stmt::DestructureDecl { .. }
            ));
        } else {
            panic!("期望解析出形参降级函数");
        }
    }

    #[test]
    fn parses_async_arrow_functions() {
        let code = r#"
            const f1 = async () => 42;
            const f2 = async (x) => { return x + 1; };
            const f3 = async x => x * 2;
            test("it", async (t) => { await t; });
        "#;
        let prog = parse(code);
        assert_eq!(prog.body.len(), 4);

        if let SpannedStmt {
            stmt:
                Stmt::VarDecl {
                    init: Some(Expr::Function(def)),
                    ..
                },
            ..
        } = &prog.body[0]
        {
            assert!(def.is_async);
            assert!(def.is_arrow);
            assert!(def.params.is_empty());
        } else {
            panic!("期望解析出 async 箭头函数 f1");
        }

        if let SpannedStmt {
            stmt:
                Stmt::VarDecl {
                    init: Some(Expr::Function(def)),
                    ..
                },
            ..
        } = &prog.body[1]
        {
            assert!(def.is_async);
            assert!(def.is_arrow);
            assert_eq!(def.params, vec!["x"]);
        } else {
            panic!("期望解析出 async 箭头函数 f2");
        }

        if let SpannedStmt {
            stmt:
                Stmt::VarDecl {
                    init: Some(Expr::Function(def)),
                    ..
                },
            ..
        } = &prog.body[2]
        {
            assert!(def.is_async);
            assert!(def.is_arrow);
            assert_eq!(def.params, vec!["x"]);
        } else {
            panic!("期望解析出 async 箭头函数 f3");
        }
    }
}
