//! 源码单元（SourceUnit）到字节码模块的贯通编译器与流水线阶段控制器。

use crate::error::CompileError;
use crate::module::{compile_esm_module, compile_module};
use aluka_bytecode::BytecodeModule;
use aluka_parser::ast::{Expr, ObjectProp, Program, PropKey, PropValue, SpannedStmt, Stmt};
use aluka_parser::source_unit::{
    ModuleKind, STAGE_BYTECODE_COMPILED, STAGE_PARSED, SourceKind, SourceUnit,
};

/// 编译源码单元为标准字节码模块，自动推进 `STAGE_BYTECODE_COMPILED` 阶段。
///
/// - 对 JavaScript / TypeScript：编译其内部 AST Program（ESM 单元按 CJS 包装
///   形态编译，支持 `require(esm)` 互操作）；
/// - 对 JSON 单元：自动将其数据结构编译为构造数据并返回的字节码模块（符合 CJS/ESM 模块规范）。
pub fn compile_source_unit(unit: &mut SourceUnit) -> Result<BytecodeModule, CompileError> {
    match unit.source_kind {
        SourceKind::JavaScript | SourceKind::TypeScript => {
            unit.require_stages(STAGE_PARSED)?;
            let program = unit.program.as_ref().ok_or_else(|| {
                CompileError::SourceUnitError("源码单元缺失 AST Program".to_owned())
            })?;
            let module = if unit.module_kind == ModuleKind::Esm {
                compile_esm_module(program)
            } else {
                compile_module(program)
            };
            unit.mark_stage(STAGE_BYTECODE_COMPILED)?;
            Ok(module)
        }
        SourceKind::Json => {
            let expr = parse_json_to_expr(&unit.source)?;
            let program = Program {
                body: vec![SpannedStmt::new(Stmt::Return(Some(expr)), 0)],
            };
            let module = compile_module(&program);
            unit.mark_stage(STAGE_BYTECODE_COMPILED)?;
            Ok(module)
        }
        SourceKind::Dsl => {
            let module = crate::dsl::compile_dsl_source(&unit.source)?;
            unit.mark_stage(STAGE_BYTECODE_COMPILED)?;
            Ok(module)
        }
    }
}

/// 将 JSON 源码解析为对应构造数据的 AST 表达式。
pub fn parse_json_to_expr(src: &str) -> Result<Expr, CompileError> {
    let mut parser = JsonParser::new(src);
    let expr = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.cursor < parser.chars.len() {
        return Err(CompileError::JsonParseError(format!(
            "JSON 数据末尾存在多余字符，位于位置 {}",
            parser.cursor
        )));
    }
    Ok(expr)
}

struct JsonParser {
    chars: Vec<char>,
    cursor: usize,
}

impl JsonParser {
    fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            cursor: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.cursor).copied()
    }

    fn next(&mut self) -> Option<char> {
        if self.cursor < self.chars.len() {
            let ch = self.chars[self.cursor];
            self.cursor += 1;
            Some(ch)
        } else {
            None
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<Expr, CompileError> {
        self.skip_whitespace();
        let ch = self
            .peek()
            .ok_or_else(|| CompileError::JsonParseError("JSON 意外终止".to_owned()))?;
        match ch {
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            '"' => self.parse_string().map(Expr::String),
            't' | 'f' => self.parse_bool(),
            'n' => self.parse_null(),
            '-' | '0'..='9' => self.parse_number(),
            _ => Err(CompileError::JsonParseError(format!(
                "JSON 无效起始字符 '{ch}'，位于位置 {}",
                self.cursor
            ))),
        }
    }

    fn parse_object(&mut self) -> Result<Expr, CompileError> {
        self.next(); // 消耗 '{'
        self.skip_whitespace();
        let mut props = Vec::new();

        if let Some('}') = self.peek() {
            self.next();
            return Ok(Expr::Object(props));
        }

        loop {
            self.skip_whitespace();
            if self.peek() != Some('"') {
                return Err(CompileError::JsonParseError(format!(
                    "JSON 对象的键必须是双引号字符串，位于位置 {}",
                    self.cursor
                )));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.next() != Some(':') {
                return Err(CompileError::JsonParseError(format!(
                    "JSON 对象缺少冒号 ':'，位于位置 {}",
                    self.cursor
                )));
            }
            let value = self.parse_value()?;
            props.push(ObjectProp {
                key: PropKey::Literal(key),
                value: PropValue::Expr(value),
            });

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.next();
                }
                Some('}') => {
                    self.next();
                    break;
                }
                _ => {
                    return Err(CompileError::JsonParseError(format!(
                        "JSON 对象键值对之间缺少逗号 ',' 或结尾缺少 '}}'，位于位置 {}",
                        self.cursor
                    )));
                }
            }
        }

        Ok(Expr::Object(props))
    }

    fn parse_array(&mut self) -> Result<Expr, CompileError> {
        self.next(); // 消耗 '['
        self.skip_whitespace();
        let mut elements = Vec::new();

        if let Some(']') = self.peek() {
            self.next();
            return Ok(Expr::Array(elements));
        }

        loop {
            let elem = self.parse_value()?;
            elements.push(elem);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.next();
                }
                Some(']') => {
                    self.next();
                    break;
                }
                _ => {
                    return Err(CompileError::JsonParseError(format!(
                        "JSON 数组元素之间缺少逗号 ',' 或结尾缺少 ']'，位于位置 {}",
                        self.cursor
                    )));
                }
            }
        }

        Ok(Expr::Array(elements))
    }

    fn parse_string(&mut self) -> Result<String, CompileError> {
        self.next(); // 消耗起始 '"'
        let mut s = String::new();

        while let Some(ch) = self.next() {
            match ch {
                '"' => return Ok(s),
                '\\' => {
                    let esc = self.next().ok_or_else(|| {
                        CompileError::JsonParseError("JSON 字符串中未完成的转义序列".to_owned())
                    })?;
                    match esc {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'b' => s.push('\x08'),
                        'f' => s.push('\x0c'),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        'u' => {
                            let mut hex = String::new();
                            for _ in 0..4 {
                                let c = self.next().ok_or_else(|| {
                                    CompileError::JsonParseError(
                                        "JSON \\u 缺少足够的 16 进制字符".to_owned(),
                                    )
                                })?;
                                hex.push(c);
                            }
                            let code = u32::from_str_radix(&hex, 16).map_err(|e| {
                                CompileError::JsonParseError(format!(
                                    "JSON 无效的 unicode 转义: {e}"
                                ))
                            })?;
                            let ch = char::from_u32(code).ok_or_else(|| {
                                CompileError::JsonParseError(format!(
                                    "JSON 无效的 unicode 标量值: {code}"
                                ))
                            })?;
                            s.push(ch);
                        }
                        other => {
                            return Err(CompileError::JsonParseError(format!(
                                "JSON 不支持的转义字符 '\\{other}'"
                            )));
                        }
                    }
                }
                other => s.push(other),
            }
        }

        Err(CompileError::JsonParseError(
            "JSON 字符串未闭合双引号".to_owned(),
        ))
    }

    fn parse_bool(&mut self) -> Result<Expr, CompileError> {
        if self.match_exact("true") {
            Ok(Expr::Boolean(true))
        } else if self.match_exact("false") {
            Ok(Expr::Boolean(false))
        } else {
            Err(CompileError::JsonParseError(format!(
                "JSON 无效的布尔值表示，位于位置 {}",
                self.cursor
            )))
        }
    }

    fn parse_null(&mut self) -> Result<Expr, CompileError> {
        if self.match_exact("null") {
            Ok(Expr::Null)
        } else {
            Err(CompileError::JsonParseError(format!(
                "JSON 无效的 null 表示，位于位置 {}",
                self.cursor
            )))
        }
    }

    fn match_exact(&mut self, expected: &str) -> bool {
        let expected_chars: Vec<char> = expected.chars().collect();
        if self.cursor + expected_chars.len() <= self.chars.len() {
            for (i, &c) in expected_chars.iter().enumerate() {
                if self.chars[self.cursor + i] != c {
                    return false;
                }
            }
            self.cursor += expected_chars.len();
            true
        } else {
            false
        }
    }

    fn parse_number(&mut self) -> Result<Expr, CompileError> {
        let start = self.cursor;
        if self.peek() == Some('-') {
            self.next();
        }

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == '.' || ch == 'e' || ch == 'E' || ch == '+' || ch == '-'
            {
                self.next();
            } else {
                break;
            }
        }

        let num_str: String = self.chars[start..self.cursor].iter().collect();
        let val: f64 = num_str.parse().map_err(|e| {
            CompileError::JsonParseError(format!("JSON 无效的数值 \"{num_str}\": {e}"))
        })?;
        Ok(Expr::Number(val))
    }
}
