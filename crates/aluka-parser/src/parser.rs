//! 递归下降语法分析器（JS / TS 源码 → AST）。
//!
//! 支持 ECMAScript 核心文法、Class、Try/Catch、可选链，以及 TypeScript 类型注解零成本剥离。

use crate::ast::{
    ArrayPatternElem, ClassMethodDef, ExportDecl, ExportSpecifier, Expr, FunctionDef, ImportDecl,
    ImportSpecifier, ObjectPatternProp, ObjectProp, Program, PropKey, PropValue, Stmt, SwitchCase,
    VarKind, VarPattern,
};
use crate::lexer::{Lexer, Token, TokenKind};

/// 语法分析器。
pub struct Parser<'src> {
    tokens: Vec<Token>,
    pos: usize,
    _src: &'src str,
    /// 语法错误收集。`parse` 维持容错（错误不阻断、AST 尽力而为，兼容
    /// 既有调用方）；`parse_strict`/`take_errors` 供 alukac 等需要拒绝
    /// 非法源码的入口使用。
    errors: Vec<String>,
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
            _src: src,
            errors: Vec::new(),
        }
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
    fn eat_semi(&mut self) {
        self.match_punct(";");
    }

    /// 跳过 TypeScript 类型注解（例如 `: number`, `: Array<string>`, `: (x: number) => void` 等）
    fn skip_type_annotation(&mut self) {
        if self.match_punct(":") {
            let mut paren_depth = 0;
            let mut angle_depth = 0;
            while self.pos < self.tokens.len() {
                let tok = self.peek();
                if let TokenKind::Punct(p) = &tok.kind {
                    match p.as_str() {
                        "(" => paren_depth += 1,
                        ")" => {
                            if paren_depth > 0 {
                                paren_depth -= 1;
                            } else {
                                break;
                            }
                        }
                        "<" => angle_depth += 1,
                        ">" if angle_depth > 0 => {
                            angle_depth -= 1;
                        }
                        "{" | "=" | ";" | "," if paren_depth == 0 && angle_depth == 0 => {
                            break;
                        }
                        _ => {}
                    }
                }
                self.advance();
            }
        }
    }

    /// 解析完整 Program
    pub fn parse_program(&mut self) -> Program {
        let mut body = Vec::new();
        while self.peek().kind != TokenKind::Eof {
            // 跳过 TS interface / type 声明
            if self.check_keyword("interface") {
                self.advance();
                // 跳过名字
                self.advance();
                // 跳过主体 `{ ... }`
                if self.match_punct("{") {
                    let mut depth = 1;
                    while depth > 0 && self.peek().kind != TokenKind::Eof {
                        if self.match_punct("{") {
                            depth += 1;
                        } else if self.match_punct("}") {
                            depth -= 1;
                        } else {
                            self.advance();
                        }
                    }
                }
                continue;
            }
            // TS 类型别名：`type Foo = ...` / `type Foo<T> = ...`。`type` 是
            // 软关键字（真实包常用作变量名），仅在后随 Ident + `=`/`<` 时
            // 按别名声明跳过
            if matches!(&self.peek().kind, TokenKind::Ident(t) if t == "type")
                && matches!(self.peek_ahead(1).kind, TokenKind::Ident(_))
                && (self.peek_ahead(2).is_punct("=") || self.peek_ahead(2).is_punct("<"))
            {
                self.advance();
                while !self.check_punct(";") && self.peek().kind != TokenKind::Eof {
                    self.advance();
                }
                self.eat_semi();
                continue;
            }

            body.push(self.parse_stmt());
        }
        Program { body }
    }

    /// 解析语句
    pub fn parse_stmt(&mut self) -> Stmt {
        if (self.peek().kind == TokenKind::Keyword("import".to_owned())
            || self.peek().kind == TokenKind::Ident("import".to_owned()))
            && !self.peek_ahead(1).is_punct("(")
            && !self.peek_ahead(1).is_punct(".")
        {
            return self.parse_import_stmt();
        }

        if self.peek().kind == TokenKind::Keyword("export".to_owned())
            || self.peek().kind == TokenKind::Ident("export".to_owned())
        {
            return self.parse_export_stmt();
        }

        if self.match_punct("{") {
            let mut stmts = Vec::new();
            while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
                stmts.push(self.parse_stmt());
            }
            let _ = self.expect_punct("}");
            return Stmt::Block(stmts);
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
            return Stmt::If {
                cond,
                then_branch,
                else_branch,
            };
        }

        if self.match_keyword("while") {
            let _ = self.expect_punct("(");
            let cond = self.parse_expr();
            let _ = self.expect_punct(")");
            let body = Box::new(self.parse_stmt());
            return Stmt::While { cond, body };
        }

        if self.match_keyword("do") {
            let body = Box::new(self.parse_stmt());
            let _ = self.match_keyword("while");
            let _ = self.expect_punct("(");
            let cond = self.parse_expr();
            let _ = self.expect_punct(")");
            self.eat_semi();
            return Stmt::DoWhile { body, cond };
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
                    return Stmt::ForIn {
                        pattern,
                        right,
                        body,
                    };
                } else {
                    if self.peek().kind == TokenKind::Keyword("of".to_owned())
                        || self.peek().kind == TokenKind::Ident("of".to_owned())
                    {
                        self.advance();
                    }
                    let right = self.parse_expr();
                    let _ = self.expect_punct(")");
                    let body = Box::new(self.parse_stmt());
                    return Stmt::ForOf {
                        is_await,
                        pattern,
                        right,
                        body,
                    };
                }
            }

            let init = if self.match_punct(";") {
                None
            } else if self.peek().kind == TokenKind::Keyword("let".to_owned())
                || self.peek().kind == TokenKind::Keyword("var".to_owned())
                || self.peek().kind == TokenKind::Keyword("const".to_owned())
            {
                Some(Box::new(self.parse_var_decl()))
            } else {
                let expr = self.parse_expr_sequence();
                let _ = self.expect_punct(";");
                Some(Box::new(Stmt::Expr(expr)))
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
            return Stmt::For {
                init,
                cond,
                update,
                body,
            };
        }

        if self.match_keyword("break") {
            self.eat_semi();
            return Stmt::Break;
        }

        if self.match_keyword("continue") {
            self.eat_semi();
            return Stmt::Continue;
        }

        if self.match_keyword("throw") {
            let expr = self.parse_expr();
            self.eat_semi();
            return Stmt::Throw(expr);
        }

        if self.match_keyword("return") {
            let expr = if self.check_punct(";")
                || self.check_punct("}")
                || self.peek().kind == TokenKind::Eof
            {
                None
            } else {
                Some(self.parse_expr())
            };
            self.eat_semi();
            return Stmt::Return(expr);
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
            return Stmt::Try {
                body,
                catch_param,
                catch_body,
                finally_body,
            };
        }

        if self.peek().kind == TokenKind::Keyword("let".to_owned())
            || self.peek().kind == TokenKind::Keyword("const".to_owned())
            || self.peek().kind == TokenKind::Keyword("var".to_owned())
        {
            return self.parse_var_decl();
        }

        if self.match_keyword("async") {
            if self.match_keyword("function") {
                let is_generator = self.match_punct("*");
                let mut def = self.parse_function_def();
                def.is_async = true;
                def.is_generator = is_generator;
                return Stmt::Function(def);
            }
            self.pos -= 1;
        }

        if self.match_keyword("function") {
            let is_generator = self.match_punct("*");
            let mut def = self.parse_function_def();
            def.is_generator = is_generator;
            return Stmt::Function(def);
        }

        if self.match_keyword("class") {
            return self.parse_class_stmt();
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
            return Stmt::Switch {
                discriminant,
                cases,
            };
        }

        // 默认作为表达式语句
        let expr = self.parse_expr();
        self.eat_semi();
        Stmt::Expr(expr)
    }

    fn parse_var_pattern(&mut self) -> VarPattern {
        if self.match_punct("[") {
            let mut elements = Vec::new();
            while !self.check_punct("]") && self.peek().kind != TokenKind::Eof {
                if self.match_punct("...") {
                    let name = if let TokenKind::Ident(id) = self.advance().kind {
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
                    });
                    break;
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

        let name = match self.advance().kind {
            TokenKind::Ident(id) => id,
            other => {
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
            let extra_name = match self.advance().kind {
                TokenKind::Ident(id) => id,
                other => {
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

    fn parse_function_def(&mut self) -> FunctionDef {
        let name = if let TokenKind::Ident(id) = self.peek().kind.clone() {
            self.advance();
            id
        } else {
            String::new()
        };
        let _ = self.expect_punct("(");
        let mut params = Vec::new();
        let mut is_var_args = false;
        let mut prologue_stmts = Vec::new();

        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
            if self.match_punct("...") {
                is_var_args = true;
            }
            if self.check_punct("[") || self.check_punct("{") {
                let pattern = self.parse_var_pattern();
                let param_name = format!("__param_{}__", params.len());
                params.push(param_name.clone());
                prologue_stmts.push(Stmt::DestructureDecl {
                    pattern,
                    init: Expr::Ident(param_name),
                });
                self.skip_type_annotation();
            } else if let TokenKind::Ident(param_name) = self.advance().kind {
                params.push(param_name.clone());
                self.skip_type_annotation();
                // 默认参数 `param = default`：运行时参数为 undefined 时取默认值
                //（对齐 Go 前端：函数体 prologue 注入条件赋值）
                if self.match_punct("=") {
                    let default_expr = self.parse_expr();
                    prologue_stmts.push(Stmt::Expr(Expr::Assign {
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
                    }));
                }
            }
            if !self.match_punct(",") {
                break;
            }
        }
        let _ = self.expect_punct(")");
        self.skip_type_annotation(); // 函数返回值类型
        let body_stmt = self.parse_stmt();
        let mut body = match body_stmt {
            Stmt::Block(stmts) => stmts,
            other => vec![other],
        };
        if !prologue_stmts.is_empty() {
            prologue_stmts.append(&mut body);
            body = prologue_stmts;
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
        let name = if let TokenKind::Ident(id) = self.advance().kind {
            id
        } else {
            "AnonymousClass".to_owned()
        };
        let super_class = if self.match_keyword("extends") {
            Some(self.parse_expr_primary())
        } else {
            None
        };

        let _ = self.expect_punct("{");
        let mut constructor = None;
        let mut methods = Vec::new();

        while !self.check_punct("}") && self.peek().kind != TokenKind::Eof {
            let is_static = self.match_keyword("static");
            let m_name = if let TokenKind::Ident(id) = self.peek().kind.clone() {
                self.advance();
                id
            } else if let TokenKind::Keyword(kw) = self.peek().kind.clone() {
                self.advance();
                kw
            } else {
                break;
            };

            let _ = self.expect_punct("(");
            let mut params = Vec::new();
            while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
                if let TokenKind::Ident(p) = self.advance().kind {
                    params.push(p);
                    self.skip_type_annotation();
                }
                if !self.match_punct(",") {
                    break;
                }
            }
            let _ = self.expect_punct(")");
            self.skip_type_annotation();
            let body_stmt = self.parse_stmt();
            let body = match body_stmt {
                Stmt::Block(stmts) => stmts,
                other => vec![other],
            };

            if m_name == "constructor" {
                constructor = Some(FunctionDef {
                    name: format!("{name}_constructor"),
                    params,
                    is_var_args: false,
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
                    kind: 0,
                });
            }
        }
        let _ = self.expect_punct("}");

        Stmt::Class {
            name,
            super_class,
            constructor,
            methods,
        }
    }

    /// 解析表达式入口
    pub fn parse_expr(&mut self) -> Expr {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Expr {
        if self.match_keyword("yield") {
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
        if self.match_keyword("await") {
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
            // 普通成员访问: obj.prop（prop 可为标识符或关键字，如 `m.default`）
            if self.match_punct(".") {
                if let TokenKind::Ident(prop) | TokenKind::Keyword(prop) = self.advance().kind {
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
            // 普通函数调用: fn(a, b)
            if self.match_punct("(") {
                let args = self.parse_args();
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                };
                continue;
            }
            // TypeScript `as Type` 断言零成本剥离
            if self.match_keyword("as") {
                // 跳过类型名
                self.advance();
                continue;
            }
            break;
        }

        // 后缀自增自减：i++ 或 i--
        if let TokenKind::Punct(p) = &self.peek().kind {
            if p == "++" || p == "--" {
                let op = p.clone();
                self.advance();
                return Expr::Update {
                    op,
                    target: Box::new(expr),
                    prefix: false,
                };
            }
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
                    "super" => Expr::Super,
                    "function" => {
                        let is_generator = self.match_punct("*");
                        let mut def = self.parse_function_def();
                        def.is_generator = is_generator;
                        Expr::Function(def)
                    }
                    "async" => {
                        if self.match_keyword("function") {
                            let is_generator = self.match_punct("*");
                            let mut def = self.parse_function_def();
                            def.is_async = true;
                            def.is_generator = is_generator;
                            Expr::Function(def)
                        } else if self.is_arrow_function() {
                            self.parse_arrow_function_from_paren(true)
                        } else if let TokenKind::Ident(id) = self.peek().kind.clone() {
                            if self.peek_ahead(1).kind == TokenKind::Punct("=>".to_owned()) {
                                self.advance(); // 消耗 id
                                self.advance(); // 消耗 =>
                                let body = self.parse_arrow_body();
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
                    let expr = self.parse_expr();
                    let _ = self.expect_punct(")");
                    expr
                }
            }
            TokenKind::Punct(p) if p == "[" => {
                self.advance();
                let mut elements = Vec::new();
                while !self.check_punct("]") && self.peek().kind != TokenKind::Eof {
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
                                Stmt::Block(stmts) => stmts,
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
                                param_name
                            } else {
                                String::new()
                            };
                            let _ = self.expect_punct(")");
                            let body_stmt = self.parse_stmt();
                            let body = match body_stmt {
                                Stmt::Block(stmts) => stmts,
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
                    let key = self.parse_prop_key();
                    if self.match_punct("(") {
                        let mut params = Vec::new();
                        let mut is_var_args = false;
                        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
                            if self.match_punct("...") {
                                is_var_args = true;
                            }
                            if let TokenKind::Ident(param_name) = self.advance().kind {
                                params.push(param_name);
                                self.skip_type_annotation();
                            }
                            if !self.match_punct(",") {
                                break;
                            }
                        }
                        let _ = self.expect_punct(")");
                        self.skip_type_annotation();
                        let body_stmt = self.parse_stmt();
                        let body = match body_stmt {
                            Stmt::Block(stmts) => stmts,
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
                                is_async: false,
                                is_generator: false,
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
                Expr::Object(props)
            }
            TokenKind::Punct(ref p) if p == "/" || p == "/=" => {
                if let Some(regex) = self.parse_regexp_literal() {
                    regex
                } else {
                    self.advance();
                    Expr::Undefined
                }
            }
            _ => {
                self.advance();
                Expr::Undefined
            }
        }
    }

    /// 解析正则表达式字面量 `/pattern/flags` 并重同步 Token 游标
    fn parse_regexp_literal(&mut self) -> Option<Expr> {
        let tok = self.peek();
        let start = tok.start;
        let bytes = self._src.as_bytes();
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

        let pattern = self._src[start + 1..idx].to_owned();
        idx += 1; // 消耗闭合 '/'

        let flags_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
            idx += 1;
        }
        let flags = self._src[flags_start..idx].to_owned();

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
                            while j < self.tokens.len()
                                && !matches!(&self.tokens[j].kind, TokenKind::Punct(p) if p == "=>" || p == ";")
                            {
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

    fn parse_arrow_body(&mut self) -> Vec<Stmt> {
        if self.check_punct("{") {
            let stmt = self.parse_stmt();
            match stmt {
                Stmt::Block(stmts) => stmts,
                other => vec![other],
            }
        } else {
            let expr = self.parse_assignment();
            vec![Stmt::Return(Some(expr))]
        }
    }

    fn parse_arrow_function_from_paren(&mut self, is_async: bool) -> Expr {
        self.advance(); // 消耗 (
        let mut params = Vec::new();
        let mut is_var_args = false;
        let mut prologue_stmts: Vec<Stmt> = Vec::new();
        while !self.check_punct(")") && self.peek().kind != TokenKind::Eof {
            if self.match_punct("...") {
                is_var_args = true;
            }
            if let TokenKind::Ident(p_name) = self.advance().kind {
                params.push(p_name.clone());
                self.skip_type_annotation();
                // 默认参数 `param = default`：与具名函数同款
                // prologue 条件赋值（undefined 时取默认值）
                if self.match_punct("=") {
                    let default_expr = self.parse_expr();
                    prologue_stmts.push(Stmt::Expr(Expr::Assign {
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
                    }));
                }
            }
            if !self.match_punct(",") {
                break;
            }
        }
        let _ = self.expect_punct(")");
        self.skip_type_annotation();
        let _ = self.expect_punct("=>");
        let mut body = self.parse_arrow_body();
        // prologue 注入到块体首部（表达式体的默认参罕见，未覆盖）
        if !prologue_stmts.is_empty() {
            if let Some(Stmt::Block(stmts)) = body.last_mut() {
                let mut all = prologue_stmts;
                all.append(stmts);
                *stmts = all;
            }
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

        // export default ...
        if self.match_keyword("default") {
            let expr = if self.match_keyword("function") {
                let is_generator = self.match_punct("*");
                let mut def = self.parse_function_def();
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
        assert!(matches!(prog.body[0], Stmt::Import(..)));
        assert!(matches!(prog.body[1], Stmt::Import(..)));
        assert!(matches!(prog.body[2], Stmt::Import(..)));
        assert!(matches!(prog.body[3], Stmt::Export(..)));
        assert!(matches!(prog.body[4], Stmt::Export(..)));
        assert!(matches!(prog.body[5], Stmt::Export(..)));
        assert!(matches!(prog.body[6], Stmt::Export(..)));
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
        if let Stmt::VarDecl {
            init: Some(Expr::TemplateLiteral { quasis, exprs }),
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
        if let Stmt::DestructureDecl {
            pattern: VarPattern::Object(props),
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
        if let Stmt::DestructureDecl {
            pattern: VarPattern::Array(elems),
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
        if let Stmt::Function(func_def) = &prog.body[3] {
            assert_eq!(func_def.params.len(), 2);
            assert_eq!(func_def.params[0], "__param_0__");
            assert_eq!(func_def.params[1], "__param_1__");
            assert!(matches!(func_def.body[0], Stmt::DestructureDecl { .. }));
            assert!(matches!(func_def.body[1], Stmt::DestructureDecl { .. }));
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

        if let Stmt::VarDecl {
            init: Some(Expr::Function(def)),
            ..
        } = &prog.body[0]
        {
            assert!(def.is_async);
            assert!(def.is_arrow);
            assert!(def.params.is_empty());
        } else {
            panic!("期望解析出 async 箭头函数 f1");
        }

        if let Stmt::VarDecl {
            init: Some(Expr::Function(def)),
            ..
        } = &prog.body[1]
        {
            assert!(def.is_async);
            assert!(def.is_arrow);
            assert_eq!(def.params, vec!["x"]);
        } else {
            panic!("期望解析出 async 箭头函数 f2");
        }

        if let Stmt::VarDecl {
            init: Some(Expr::Function(def)),
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
