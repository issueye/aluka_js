//! 极简 S-expression 领域特定语言（Aluka-S-DSL）前端解析与字节码生成器。
//!
//! 用于验证 Aluka 统一字节码 ISA 规范的跨语言与可插拔特性。
//! 该模块完全独立于 JavaScript/TypeScript 语法规则，直接根据 S-Expr 语法树
//! 编译发射标准 ALUKABC1（Version 30）字节码，生成的模块可通过
//! `aluka-bytecode::verifier` 安全检验并在 Rust VM 与 Go VM 上一致执行。

use crate::error::CompileError;
use aluka_bytecode::{BytecodeModule, Constant, FuncTemplate, Instr, Op};
use std::collections::HashMap;

/// S 表达式语法节点。
#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    /// 空值（nil / undefined）
    Nil,
    /// 布尔值
    Bool(bool),
    /// 数值
    Number(f64),
    /// 字符串
    String(String),
    /// 符号（标识符、关键字或运算符）
    Symbol(String),
    /// 复合列表 `(item1 item2 ...)`
    List(Vec<SExpr>),
}

/// S 表达式词法记号。
#[derive(Debug, Clone, PartialEq)]
enum Token {
    OpenParen,
    CloseParen,
    Number(f64),
    String(String),
    Symbol(String),
    Bool(bool),
    Nil,
}

/// 将 DSL 源代码字符串词法解析为 Token 流。
fn tokenize(src: &str) -> Result<Vec<Token>, CompileError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // 单行注释：;;
        if c == ';' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '(' {
            tokens.push(Token::OpenParen);
            i += 1;
            continue;
        }
        if c == ')' {
            tokens.push(Token::CloseParen);
            i += 1;
            continue;
        }
        // 字符串字面量
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < n && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < n {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        other => s.push(other),
                    }
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= n {
                return Err(CompileError::SourceUnitError(
                    "DSL 语法错误：未闭合的字符串字面量".to_owned(),
                ));
            }
            i += 1; // 消费闭合双引号
            tokens.push(Token::String(s));
            continue;
        }

        // 数字或符号
        let start = i;
        let is_neg_num = c == '-' && i + 1 < n && chars[i + 1].is_ascii_digit();
        if c.is_ascii_digit() || is_neg_num {
            if is_neg_num {
                i += 1;
            }
            let mut has_dot = false;
            while i < n && (chars[i].is_ascii_digit() || (!has_dot && chars[i] == '.')) {
                if chars[i] == '.' {
                    has_dot = true;
                }
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            let val: f64 = num_str.parse().map_err(|e| {
                CompileError::SourceUnitError(format!("DSL 数值格式错误 '{num_str}': {e}"))
            })?;
            tokens.push(Token::Number(val));
            continue;
        }

        // 符号标识符
        while i < n
            && !chars[i].is_whitespace()
            && chars[i] != '('
            && chars[i] != ')'
            && chars[i] != ';'
        {
            i += 1;
        }
        let sym: String = chars[start..i].iter().collect();
        match sym.as_str() {
            "true" => tokens.push(Token::Bool(true)),
            "false" => tokens.push(Token::Bool(false)),
            "nil" | "undefined" => tokens.push(Token::Nil),
            _ => tokens.push(Token::Symbol(sym)),
        }
    }

    Ok(tokens)
}

/// 解析 Token 流为 S 表达式列表。
fn parse_sexpr_list(tokens: &[Token], pos: &mut usize) -> Result<Vec<SExpr>, CompileError> {
    let mut exprs = Vec::new();
    while *pos < tokens.len() {
        if tokens[*pos] == Token::CloseParen {
            break;
        }
        exprs.push(parse_one_sexpr(tokens, pos)?);
    }
    Ok(exprs)
}

/// 解析单个 S 表达式。
fn parse_one_sexpr(tokens: &[Token], pos: &mut usize) -> Result<SExpr, CompileError> {
    if *pos >= tokens.len() {
        return Err(CompileError::SourceUnitError(
            "DSL 语法错误：意外的输入结尾".to_owned(),
        ));
    }
    match &tokens[*pos] {
        Token::OpenParen => {
            *pos += 1;
            let list = parse_sexpr_list(tokens, pos)?;
            if *pos >= tokens.len() || tokens[*pos] != Token::CloseParen {
                return Err(CompileError::SourceUnitError(
                    "DSL 语法错误：缺少匹配的闭括号 ')'".to_owned(),
                ));
            }
            *pos += 1;
            Ok(SExpr::List(list))
        }
        Token::CloseParen => Err(CompileError::SourceUnitError(
            "DSL 语法错误：未配对的多余闭括号 ')'".to_owned(),
        )),
        Token::Number(n) => {
            let val = *n;
            *pos += 1;
            Ok(SExpr::Number(val))
        }
        Token::String(s) => {
            let val = s.clone();
            *pos += 1;
            Ok(SExpr::String(val))
        }
        Token::Bool(b) => {
            let val = *b;
            *pos += 1;
            Ok(SExpr::Bool(val))
        }
        Token::Nil => {
            *pos += 1;
            Ok(SExpr::Nil)
        }
        Token::Symbol(s) => {
            let val = s.clone();
            *pos += 1;
            Ok(SExpr::Symbol(val))
        }
    }
}

/// 编译单元上下文。
#[derive(Debug, Default)]
struct DslCompiledUnit {
    code: Vec<Instr>,
    constants: Vec<Constant>,
    locals: usize,
    symbols: HashMap<String, usize>,
    max_stack: usize,
    current_stack: usize,
}

impl DslCompiledUnit {
    fn new(num_locals: usize) -> Self {
        Self {
            locals: num_locals,
            max_stack: 16,
            ..Default::default()
        }
    }

    fn push_stack(&mut self) {
        self.current_stack += 1;
        if self.current_stack > self.max_stack {
            self.max_stack = self.current_stack;
        }
    }

    fn pop_stack(&mut self) {
        if self.current_stack > 0 {
            self.current_stack -= 1;
        }
    }

    fn add_constant(&mut self, c: Constant) -> u32 {
        if let Some(pos) = self.constants.iter().position(|it| it == &c) {
            pos as u32
        } else {
            let idx = self.constants.len() as u32;
            self.constants.push(c);
            idx
        }
    }

    fn add_string(&mut self, s: &str) -> u32 {
        self.add_constant(Constant::String(s.to_owned()))
    }
}

/// DSL 编译器。
#[derive(Debug, Default)]
pub struct DslCompiler {
    functions: Vec<FuncTemplate>,
}

impl DslCompiler {
    /// 创建新的 DSL 编译器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 编译 DSL 源码文本为标准的 ALUKABC1（Version 30）字节码模块。
    pub fn compile(&mut self, source: &str) -> Result<BytecodeModule, CompileError> {
        let tokens = tokenize(source)?;
        let mut pos = 0;
        let mut expressions = Vec::new();
        while pos < tokens.len() {
            expressions.push(parse_one_sexpr(&tokens, &mut pos)?);
        }

        // 顶层主函数槽位 locals[0] 保留为 this（与 Go VM 及 ALUKABC1 契约完全对齐）
        let mut top_unit = DslCompiledUnit::new(1);

        for expr in &expressions {
            self.compile_expr(expr, &mut top_unit)?;
            // 顶层表达式如果产生了值，弹栈丢弃（保证语句栈平衡）
            if !matches!(expr, SExpr::List(list) if !list.is_empty() && matches!(&list[0], SExpr::Symbol(s) if s == "def" || s == "fn"))
            {
                top_unit.code.push(Instr::new(Op::Pop, 0));
                top_unit.pop_stack();
            }
        }

        // 顶层末尾返回 undefined
        top_unit.code.push(Instr::new(Op::ReturnUndef, 0));

        let main_func = FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: top_unit.locals.max(1) as u32,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: top_unit.code,
            max_stack: top_unit.max_stack.max(16) as u32,
            source_file: "dsl_compiled.adsl".to_owned(),
            constants: top_unit.constants,
            upvalues: Vec::new(),
            try_table: Vec::new(),
        };

        let mut all_functions = vec![main_func];
        all_functions.extend(std::mem::take(&mut self.functions));

        let module = BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: all_functions,
            classes: Vec::new(),
        };

        // 进行 Verifier 静态安全断言
        module.verify().map_err(|e| {
            CompileError::SourceUnitError(format!("DSL 编译产物未通过字节码验证: {e}"))
        })?;

        Ok(module)
    }

    /// 递归编译单个 S 表达式为字节码指令。
    fn compile_expr(
        &mut self,
        expr: &SExpr,
        unit: &mut DslCompiledUnit,
    ) -> Result<(), CompileError> {
        match expr {
            SExpr::Nil => {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
                unit.push_stack();
                Ok(())
            }
            SExpr::Bool(b) => {
                if *b {
                    unit.code.push(Instr::new(Op::PushTrue, 0));
                } else {
                    unit.code.push(Instr::new(Op::PushFalse, 0));
                }
                unit.push_stack();
                Ok(())
            }
            SExpr::Number(n) => {
                if n.fract() == 0.0 && *n >= 0.0 && *n <= 16_777_215.0 {
                    unit.code.push(Instr::new(Op::PushInt, *n as u32));
                } else {
                    let idx = unit.add_constant(Constant::Number(*n));
                    unit.code.push(Instr::new(Op::PushConst, idx));
                }
                unit.push_stack();
                Ok(())
            }
            SExpr::String(s) => {
                let idx = unit.add_string(s);
                unit.code.push(Instr::new(Op::PushConst, idx));
                unit.push_stack();
                Ok(())
            }
            SExpr::Symbol(sym) => {
                if let Some(&slot) = unit.symbols.get(sym) {
                    unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                } else {
                    let idx = unit.add_string(sym);
                    unit.code.push(Instr::new(Op::LoadGlobal, idx));
                }
                unit.push_stack();
                Ok(())
            }
            SExpr::List(list) => {
                if list.is_empty() {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                    unit.push_stack();
                    return Ok(());
                }

                // 首项通常为运算符、关键字或被调函数
                let head = &list[0];
                if let SExpr::Symbol(sym) = head {
                    match sym.as_str() {
                        // 变量定义: (def name value)
                        "def" => {
                            if list.len() != 3 {
                                return Err(CompileError::SourceUnitError(
                                    "DSL 语法错误：(def <name> <value>) 必须包含恰好两个参数"
                                        .to_owned(),
                                ));
                            }
                            let name = match &list[1] {
                                SExpr::Symbol(s) => s.clone(),
                                _ => {
                                    return Err(CompileError::SourceUnitError(
                                        "DSL (def <name> ...) 中变量名必须为标识符".to_owned(),
                                    ));
                                }
                            };
                            self.compile_expr(&list[2], unit)?;
                            let slot = if let Some(&s) = unit.symbols.get(&name) {
                                s
                            } else {
                                let s = unit.locals;
                                unit.locals += 1;
                                unit.symbols.insert(name, s);
                                s
                            };
                            unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                            unit.pop_stack();
                            return Ok(());
                        }

                        // 变量赋值: (set! name value)
                        "set!" => {
                            if list.len() != 3 {
                                return Err(CompileError::SourceUnitError(
                                    "DSL 语法错误：(set! <name> <value>) 必须包含恰好两个参数"
                                        .to_owned(),
                                ));
                            }
                            let name = match &list[1] {
                                SExpr::Symbol(s) => s.clone(),
                                _ => {
                                    return Err(CompileError::SourceUnitError(
                                        "DSL (set! <name> ...) 中变量名必须为标识符".to_owned(),
                                    ));
                                }
                            };
                            self.compile_expr(&list[2], unit)?;
                            if let Some(&slot) = unit.symbols.get(&name) {
                                unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                            } else {
                                let name_idx = unit.add_string(&name);
                                unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                            }
                            unit.pop_stack();
                            unit.code.push(Instr::new(Op::PushUndefined, 0));
                            unit.push_stack();
                            return Ok(());
                        }

                        // 函数定义: (fn name (p1 p2 ...) body...)
                        "fn" => {
                            if list.len() < 4 {
                                return Err(CompileError::SourceUnitError(
                                    "DSL 语法错误：(fn <name> (<params>...) <body>...) 参数过少"
                                        .to_owned(),
                                ));
                            }
                            let fn_name = match &list[1] {
                                SExpr::Symbol(s) => s.clone(),
                                _ => {
                                    return Err(CompileError::SourceUnitError(
                                        "DSL (fn <name> ...) 函数名必须为标识符".to_owned(),
                                    ));
                                }
                            };
                            let params = match &list[2] {
                                SExpr::List(ps) => {
                                    let mut param_names = Vec::new();
                                    for p in ps {
                                        if let SExpr::Symbol(s) = p {
                                            param_names.push(s.clone());
                                        } else {
                                            return Err(CompileError::SourceUnitError(
                                                "DSL 函数形参必须为符号标识符".to_owned(),
                                            ));
                                        }
                                    }
                                    param_names
                                }
                                _ => {
                                    return Err(CompileError::SourceUnitError(
                                        "DSL 函数形参列表必须为列表形式".to_owned(),
                                    ));
                                }
                            };

                            let child_idx = self.compile_function(&fn_name, &params, &list[3..])?;

                            // 主函数中分配槽位，创建闭包并赋值给局部变量
                            let slot = if let Some(&s) = unit.symbols.get(&fn_name) {
                                s
                            } else {
                                let s = unit.locals;
                                unit.locals += 1;
                                unit.symbols.insert(fn_name, s);
                                s
                            };
                            unit.code
                                .push(Instr::new(Op::MakeClosure, child_idx as u32));
                            unit.push_stack();
                            unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                            unit.pop_stack();
                            return Ok(());
                        }

                        // 条件分支: (if cond then_expr else_expr)
                        "if" => {
                            if list.len() != 4 {
                                return Err(CompileError::SourceUnitError(
                                    "DSL (if <cond> <then> <else>) 必须包含三个参数".to_owned(),
                                ));
                            }
                            // 编译条件判断
                            self.compile_expr(&list[1], unit)?;
                            let jmp_false_idx = unit.code.len();
                            unit.code.push(Instr::new(Op::JmpFalsePop, 0));
                            unit.pop_stack();

                            // 编译 then 分支
                            self.compile_expr(&list[2], unit)?;
                            let jmp_end_idx = unit.code.len();
                            unit.code.push(Instr::new(Op::Jmp, 0));

                            // 回填 false 跳转（跳到 else 分支入口）
                            let else_start_idx = unit.code.len();
                            let false_offset =
                                (else_start_idx as i32 - (jmp_false_idx as i32 + 1)) * 4;
                            unit.code[jmp_false_idx].operand = (false_offset as u32) & 0x00FF_FFFF;

                            // 编译 else 分支
                            self.compile_expr(&list[3], unit)?;

                            // 回填 then 末尾的有条件跳转跳到结尾
                            let end_idx = unit.code.len();
                            let end_offset = (end_idx as i32 - (jmp_end_idx as i32 + 1)) * 4;
                            unit.code[jmp_end_idx].operand = (end_offset as u32) & 0x00FF_FFFF;

                            return Ok(());
                        }

                        // 复合语句块: (do expr1 expr2 ...)
                        "do" => {
                            if list.len() == 1 {
                                unit.code.push(Instr::new(Op::PushUndefined, 0));
                                unit.push_stack();
                                return Ok(());
                            }
                            for (i, sub) in list[1..].iter().enumerate() {
                                self.compile_expr(sub, unit)?;
                                if i < list.len() - 2 {
                                    unit.code.push(Instr::new(Op::Pop, 0));
                                    unit.pop_stack();
                                }
                            }
                            return Ok(());
                        }

                        // 二元算术与比较运算符
                        "+" | "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=" | "==" | "!=" => {
                            if list.len() != 3 {
                                return Err(CompileError::SourceUnitError(format!(
                                    "DSL 二元运算符 '{sym}' 必须有两个操作数"
                                )));
                            }
                            self.compile_expr(&list[1], unit)?;
                            self.compile_expr(&list[2], unit)?;
                            let op = match sym.as_str() {
                                "+" => Op::Add,
                                "-" => Op::Sub,
                                "*" => Op::Mul,
                                "/" => Op::Div,
                                "%" => Op::Mod,
                                "<" => Op::Lt,
                                "<=" => Op::Le,
                                ">" => Op::Gt,
                                ">=" => Op::Ge,
                                "==" => Op::StrictEq,
                                "!=" => Op::StrictNe,
                                _ => unreachable!(),
                            };
                            unit.code.push(Instr::new(op, 0));
                            unit.pop_stack(); // 弹出右操作数，保留结果
                            return Ok(());
                        }

                        // 特化支持 console.log: (console.log arg1 arg2 ...)
                        "console.log" => {
                            let g_idx = unit.add_string("console");
                            unit.code.push(Instr::new(Op::LoadGlobal, g_idx));
                            unit.push_stack();
                            let log_name_idx = unit.add_string("log");
                            let args = &list[1..];
                            for arg in args {
                                self.compile_expr(arg, unit)?;
                            }
                            // CALL_METHOD 操作数打包: args_len << 16 | method_idx
                            let packed = ((args.len() as u32) << 16) | (log_name_idx & 0xFFFF);
                            unit.code.push(Instr::new(Op::CallMethod, packed));
                            for _ in 0..args.len() {
                                unit.pop_stack();
                            }
                            // 替换 receiver 为调用返回值（undefined）
                            return Ok(());
                        }

                        _ => {}
                    }
                }

                // 普通函数调用: (callee arg1 arg2 ...)
                self.compile_expr(head, unit)?;
                let args = &list[1..];
                for arg in args {
                    self.compile_expr(arg, unit)?;
                }
                unit.code.push(Instr::new(Op::Call, args.len() as u32));
                for _ in 0..args.len() {
                    unit.pop_stack();
                }
                // callee 替换为返回值
                Ok(())
            }
        }
    }

    /// 编译 DSL 函数定义为独立的子函数模板。
    fn compile_function(
        &mut self,
        name: &str,
        params: &[String],
        body: &[SExpr],
    ) -> Result<usize, CompileError> {
        // slot 0 保留给 this，形参依次从 slot 1 开始分配
        let mut sub_unit = DslCompiledUnit::new(1 + params.len());
        for (i, p) in params.iter().enumerate() {
            sub_unit.symbols.insert(p.clone(), 1 + i);
        }

        if body.is_empty() {
            sub_unit.code.push(Instr::new(Op::ReturnUndef, 0));
        } else {
            for (i, expr) in body.iter().enumerate() {
                self.compile_expr(expr, &mut sub_unit)?;
                if i == body.len() - 1 {
                    // 最后一个表达式的值作为函数的返回值
                    sub_unit.code.push(Instr::new(Op::Return, 0));
                    sub_unit.pop_stack();
                } else {
                    sub_unit.code.push(Instr::new(Op::Pop, 0));
                    sub_unit.pop_stack();
                }
            }
        }

        let tmpl = FuncTemplate {
            name: name.to_owned(),
            num_params: params.len() as u32,
            num_locals: sub_unit.locals.max(1 + params.len()) as u32,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: sub_unit.code,
            max_stack: sub_unit.max_stack.max(16) as u32,
            source_file: "dsl_compiled.adsl".to_owned(),
            constants: sub_unit.constants,
            upvalues: Vec::new(),
            try_table: Vec::new(),
        };

        let fn_idx = self.functions.len() + 1; // +1 因为 0 保留给 main
        self.functions.push(tmpl);
        Ok(fn_idx)
    }
}

/// 编译指定 DSL 源码为标准字节码模块（便捷函数）。
pub fn compile_dsl_source(source: &str) -> Result<BytecodeModule, CompileError> {
    let mut compiler = DslCompiler::new();
    compiler.compile(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsl_arithmetic_and_variables() {
        let src = r#"
            ;; 定义变量
            (def a 10)
            (def b 20)
            ;; 计算并打印
            (console.log "result:" (+ a (* b 2)))
        "#;
        let module = compile_dsl_source(src).expect("DSL 源码应编译成功");
        assert_eq!(module.version, 30);
        assert_eq!(module.functions.len(), 1);
        let main = &module.functions[0];
        assert_eq!(main.name, "main");
        // 校验包含算术指令与方法调用
        assert!(main.code.iter().any(|i| i.op == Op::Add));
        assert!(main.code.iter().any(|i| i.op == Op::Mul));
        assert!(main.code.iter().any(|i| i.op == Op::CallMethod));
    }

    #[test]
    fn test_dsl_function_definition_and_calling() {
        let src = r#"
            (fn add (x y)
                (+ x y))
            (def res (add 3 5))
            (console.log "add result:" res)
        "#;
        let module = compile_dsl_source(src).expect("DSL 函数应编译成功");
        assert_eq!(module.functions.len(), 2);
        assert_eq!(module.functions[1].name, "add");
        assert_eq!(module.functions[1].num_params, 2);
    }

    #[test]
    fn test_dsl_conditional_branching() {
        let src = r#"
            (def x 15)
            (if (> x 10)
                (console.log "greater")
                (console.log "smaller"))
        "#;
        let module = compile_dsl_source(src).expect("DSL 条件分支应编译成功");
        let main = &module.functions[0];
        assert!(main.code.iter().any(|i| i.op == Op::JmpFalsePop));
        assert!(main.code.iter().any(|i| i.op == Op::Jmp));
    }
}
