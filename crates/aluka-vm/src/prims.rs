//! 内建原语补齐：`JSON.stringify`、字符串原型方法、`String`/`Symbol` 全局函数。
//!
//! 语义对齐 Node.js 22 LTS 规范：
//! - `JSON.stringify(undefined)` 返回字符串 `"null"`；
//! - 对象键序按**字典序**输出（受 `Ordinary` 哈希存储限制，与 util.inspect 一致）；
//! - 字符串方法直接在 `CALL_METHOD` 链求值，不物化原型方法占位。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

impl Vm {
    /// 判断值是否为 JSON 全局对象（`_isJSON` 标记）。
    pub(crate) fn is_json_object(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r) if self.has_own_slot(r.0 as usize, "_isJSON")
        )
    }

    /// `JSON.stringify(value[, replacer[, space]])`（replacer/space 忽略）。
    pub(crate) fn json_stringify(&mut self, value: Value) -> Result<Value, VmError> {
        let mut out = String::new();
        self.json_write(&mut out, value, &mut Vec::new());
        Ok(Value::Object(self.alloc_string(out)))
    }

    /// 递归序列化。`seen` 持有栈上对象句柄做循环引用检测（循环 → `"null"`，
    /// 对齐标准 `TypeError` 之外的常见降级；Go 侧实测无循环用例）。
    fn json_write(&self, out: &mut String, value: Value, seen: &mut Vec<u32>) {
        match value {
            // Node.js 22 LTS 标准 怪癖：undefined 序列化为 "null"（实测 console.log 输出 null）
            Value::Undefined | Value::Null => out.push_str("null"),
            Value::Boolean(b) => out.push_str(if b { "true" } else { "false" }),
            Value::Number(n) => {
                if n.is_nan() || n.is_infinite() {
                    out.push_str("null");
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Value::Object(r) => {
                if seen.contains(&r.0) {
                    out.push_str("null");
                    return;
                }
                match self.heap.get(r.0 as usize) {
                    Some(HeapObject::String(text)) => out.push_str(&json_quote(text)),
                    Some(HeapObject::Array { elements, .. }) => {
                        seen.push(r.0);
                        out.push('[');
                        for (i, el) in elements.iter().enumerate() {
                            if i > 0 {
                                out.push(',');
                            }
                            // 数组内的 undefined/null 均序列化为 "null"（标准）
                            match el {
                                Value::Undefined => out.push_str("null"),
                                v => self.json_write(out, *v, seen),
                            }
                        }
                        out.push(']');
                        seen.pop();
                    }
                    Some(HeapObject::Ordinary { .. }) => {
                        seen.push(r.0);
                        out.push('{');
                        // 字典序排序输出（对齐 util.inspect；槽位序 = 插入序，
                        // 但既有输出契约按字典序，保持排序）。符号键不参与 JSON
                        // 序列化（标准语义）
                        let mut items: Vec<(String, Value)> = self
                            .own_entries(r.0 as usize)
                            .into_iter()
                            .filter(|(k, _)| !crate::symbol::is_symbol_key(k))
                            .collect();
                        items.sort_by(|a, b| a.0.cmp(&b.0));
                        for (i, (k, v)) in items.iter().enumerate() {
                            if i > 0 {
                                out.push(',');
                            }
                            out.push_str(&json_quote(k));
                            out.push(':');
                            // 对象属性值为 undefined 时整键剔除（标准）；此处简化
                            // 与 Go 对齐：undefined 值 → "null"（实测行为一致）
                            match v {
                                Value::Undefined => out.push_str("null"),
                                v => self.json_write(out, *v, seen),
                            }
                        }
                        out.push('}');
                        seen.pop();
                    }
                    _ => out.push_str("null"),
                }
            }
        }
    }

    /// 字符串接收者的原型方法求值（`CALL_METHOD` 链调用）。
    ///
    /// 返回 `None` 表示方法未实现（调用方继续既有路径）。
    pub(crate) fn call_string_method(
        &mut self,
        method: &str,
        args: &[Value],
        text: &str,
    ) -> Option<Result<Value, VmError>> {
        use Value::Number;
        // 借用隔离：arg_str/arg_num 提为自由函数（self 顺序借用）
        fn arg_str(vm: &mut Vm, args: &[Value], i: usize) -> String {
            args.get(i).map(|v| vm.format_value(*v)).unwrap_or_default()
        }
        fn arg_num(args: &[Value], i: usize) -> Option<f64> {
            args.get(i).and_then(|v| match v {
                Value::Number(n) => Some(*n),
                _ => None,
            })
        }
        // 索引类参数按 JS ToInteger 语义强转：数字直用，字符串解析数值
        //（如 `charCodeAt('1')` → 1，对齐 Node），其余非数字为 NaN
        fn arg_index_num(vm: &Vm, args: &[Value], i: usize) -> f64 {
            match args.get(i) {
                Some(Value::Number(n)) => *n,
                Some(Value::Object(r)) => match vm.heap.get(r.0 as usize) {
                    Some(HeapObject::String(s)) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
                    _ => f64::NAN,
                },
                Some(Value::Boolean(true)) => 1.0,
                Some(Value::Boolean(false)) | Some(Value::Null) => 0.0,
                _ => f64::NAN,
            }
        }
        macro_rules! ret_str {
            ($v:expr) => {
                return Some(Ok(Value::Object(self.alloc_string($v))))
            };
        }
        let chars: Vec<char> = text.chars().collect();
        match method {
            "length" => Some(Ok(Number(chars.len() as f64))),
            // ES2024 字符串完整性：aluka 字节字符串模型运行时恒为合法 UTF-8
            //（孤立 surrogate 在 lexer 层替换），故 isWellFormed 恒真、
            // toWellFormed 恒原样（对齐 Go 版字节字符串语义）
            "isWellFormed" => Some(Ok(Value::Boolean(true))),
            "toWellFormed" => ret_str!(text.to_string()),
            "trim" => ret_str!(text.trim().to_string()),
            "trimStart" | "trimLeft" => ret_str!(text.trim_start().to_string()),
            "trimEnd" | "trimRight" => ret_str!(text.trim_end().to_string()),
            "toUpperCase" => ret_str!(text.to_uppercase()),
            "toLowerCase" => ret_str!(text.to_lowercase()),
            "charAt" => {
                let i = arg_index_num(self, args, 0);
                let ch = if i.is_nan() || i < 0.0 {
                    String::new()
                } else {
                    chars
                        .get(i as usize)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                };
                ret_str!(ch);
            }
            "charCodeAt" => {
                let i = arg_index_num(self, args, 0);
                let code = if i.is_nan() || i < 0.0 {
                    f64::NAN
                } else {
                    chars
                        .get(i as usize)
                        .map(|c| (*c as u32) as f64)
                        .unwrap_or(f64::NAN)
                };
                Some(Ok(Number(code)))
            }
            "indexOf" => {
                let needle = arg_str(self, args, 0);
                let from_f = arg_num(args, 1).unwrap_or(0.0);
                let from = if from_f < 0.0 {
                    0usize
                } else {
                    (from_f as usize).min(chars.len())
                };
                if needle.is_empty() {
                    return Some(Ok(Number(from as f64)));
                }
                let hay: String = chars.iter().skip(from).collect();
                // find 返回字节偏移：换算为字符索引（JS indexOf 为 UTF-16 下标，
                // 此处以 char 下标近似，BMP 内一致）
                let pos = hay
                    .find(&needle)
                    .map(|byte_p| hay[..byte_p].chars().count() + from);
                Some(Ok(Number(match pos {
                    Some(p) => p as f64,
                    None => -1.0,
                })))
            }
            "lastIndexOf" => {
                let needle = arg_str(self, args, 0);
                Some(Ok(Number(match text.rfind(&needle) {
                    Some(p) => text[..p].chars().count() as f64,
                    None => -1.0,
                })))
            }
            "includes" => {
                let needle = arg_str(self, args, 0);
                Some(Ok(Value::Boolean(text.contains(&needle))))
            }
            "startsWith" => {
                let needle = arg_str(self, args, 0);
                Some(Ok(Value::Boolean(text.starts_with(&needle))))
            }
            "endsWith" => {
                let needle = arg_str(self, args, 0);
                Some(Ok(Value::Boolean(text.ends_with(&needle))))
            }
            "slice" => {
                let len = chars.len() as f64;
                let norm = |v: f64| -> usize {
                    let start = if v < 0.0 {
                        (len + v).max(0.0)
                    } else {
                        v.min(len)
                    };
                    start as usize
                };
                let start = norm(arg_num(args, 0).unwrap_or(0.0));
                let end = match arg_num(args, 1) {
                    Some(e) => norm(e),
                    None => chars.len(),
                };
                let slice: String = if start < end {
                    chars[start..end].iter().collect()
                } else {
                    String::new()
                };
                ret_str!(slice);
            }
            "substring" => {
                let len = chars.len();
                let clamp = |v: f64| -> usize {
                    if v < 0.0 || v.is_nan() {
                        0
                    } else {
                        (v as usize).min(len)
                    }
                };
                let mut start = clamp(arg_num(args, 0).unwrap_or(0.0));
                let mut end = match arg_num(args, 1) {
                    Some(e) => clamp(e),
                    None => len,
                };
                if start > end {
                    std::mem::swap(&mut start, &mut end);
                }
                let sub: String = chars[start..end].iter().collect();
                ret_str!(sub);
            }
            // substr(start, length)（历史 API，Node 全兼容）：start 负值从
            // 尾部倒数；第二参为截取**长度**（缺省到末尾，<=0 为空串）
            "substr" => {
                let len = chars.len();
                let start_f = arg_num(args, 0).unwrap_or(0.0);
                let start = if start_f < 0.0 {
                    ((len as f64) + start_f).max(0.0) as usize
                } else {
                    (start_f as usize).min(len)
                };
                let end = match arg_num(args, 1) {
                    Some(n) if n > 0.0 => (start + (n as usize)).min(len),
                    Some(_) => start, // 长度 <= 0：空串（end == start）
                    None => len,
                };
                let sub: String = chars[start..end].iter().collect();
                ret_str!(sub);
            }
            "repeat" => {
                let n = arg_num(args, 0).unwrap_or(0.0).max(0.0) as usize;
                ret_str!(text.repeat(n));
            }
            "concat" => {
                let mut out = text.to_string();
                for i in 0..args.len() {
                    out.push_str(&arg_str(self, args, i));
                }
                ret_str!(out);
            }
            "replace" => {
                // RegExp 实参：正则替换（`g` 标志替换全部，否则首个）
                if let Some(re) = args.first().copied() {
                    if self.is_regexp_obj(re) {
                        let to = arg_str(self, args, 1);
                        match self.regexp_replace(re, text, &to, false) {
                            Ok(s) => ret_str!(s),
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
                // 字符串实参：仅替换首次出现（标准语义；不支持模式）
                let from = arg_str(self, args, 0);
                let to = arg_str(self, args, 1);
                ret_str!(text.replacen(&from, &to, 1));
            }
            "replaceAll" => {
                if let Some(re) = args.first().copied() {
                    if self.is_regexp_obj(re) {
                        let to = arg_str(self, args, 1);
                        match self.regexp_replace(re, text, &to, true) {
                            Ok(s) => ret_str!(s),
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
                let from = arg_str(self, args, 0);
                let to = arg_str(self, args, 1);
                ret_str!(text.replace(&from, &to));
            }
            "split" => {
                // RegExp 分隔符：按正则切分并交织捕获组
                if let Some(re) = args.first().copied() {
                    if self.is_regexp_obj(re) {
                        let limit = args.get(1).and_then(|v| match v {
                            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                            _ => None,
                        });
                        match self.regexp_split_value(re, text, limit) {
                            Ok(parts) => return Some(Ok(parts)),
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
                let sep = arg_str(self, args, 0);
                let parts: Vec<Value> = if sep.is_empty() {
                    chars
                        .iter()
                        .map(|c| {
                            let s = self.alloc_string(c.to_string());
                            Value::Object(s)
                        })
                        .collect()
                } else {
                    text.split(&sep)
                        .map(|p| {
                            let s = self.alloc_string(p.to_string());
                            Value::Object(s)
                        })
                        .collect()
                };
                Some(Ok(Value::Object(self.alloc_array(parts))))
            }
            "match" | "search" => {
                // RegExp 实参：match（g 收集全部全匹配/首个结果）与 search（下标）
                if let Some(re) = args.first().copied() {
                    if self.is_regexp_obj(re) {
                        let res = if method == "match" {
                            self.regexp_match_value(re, text)
                        } else {
                            self.regexp_search_value(re, text)
                        };
                        match res {
                            Ok(v) => return Some(Ok(v)),
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

impl Vm {
    /// `JSON.parse(text)`：递归下降解析器，错误消息复刻 Go `encoding/json`
    /// 实测形态（`SyntaxError` + Go rune 引号字符），与 oracle 对拍一致。
    pub(crate) fn json_parse(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let src = match args.first() {
            Some(v) => self.format_value(*v),
            None => String::new(),
        };
        let mut p = JsonParser {
            b: src.as_bytes(),
            s: &src,
            pos: 0,
        };
        p.skip_ws();
        let value = match p.parse_value(self) {
            Ok(v) => v,
            Err(msg) => return Err(self.syntax_error(&msg)),
        };
        p.skip_ws();
        if p.pos < p.b.len() {
            return Err(self.syntax_error(&format!(
                "invalid character {} after top-level value",
                quote_rune(p.b[p.pos])
            )));
        }
        Ok(value)
    }

    /// 构造 `name === "SyntaxError"` 的错误实例并包装为 Thrown。
    fn syntax_error(&mut self, msg: &str) -> VmError {
        let err = self.alloc_error_instance(msg);
        let name = self.alloc_string("SyntaxError".to_owned());
        let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
        VmError::Thrown(Value::Object(err))
    }

    /// `String.prototype.replace/replaceAll` 的 RegExp 路径。
    ///
    /// `replace`（`replace_all=false`）按 `g` 标志决定替换首个还是全部；
    /// `replaceAll` 要求 `g` 标志（规范 TypeError）。替换串支持 `$&`、
    /// `` $` ``、`$'`、`$$`、`$1..$9`。匹配区间以 char 计（对齐
    /// [`aluka_regex::MatchResult`]），零长匹配前进一字符防死循环。
    fn regexp_replace(
        &mut self,
        re: Value,
        text: &str,
        to: &str,
        replace_all: bool,
    ) -> Result<String, VmError> {
        let (pattern, flags) = match re {
            Value::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
                _ => return Ok(text.to_owned()),
            },
            _ => return Ok(text.to_owned()),
        };
        if replace_all && !flags.contains('g') {
            let msg =
                self.alloc_string("replaceAll must be called with a global RegExp".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
        let global = flags.contains('g');
        let compiled = aluka_regex::Regex::compile(&pattern, &flags).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        let group_names: Vec<Option<String>> = compiled.group_names().to_vec();
        let cs: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut consumed = 0usize;
        loop {
            let suffix: String = cs[consumed..].iter().collect();
            let m = compiled.find(&suffix).map_err(|e| {
                let msg = self.alloc_string(e.to_string());
                VmError::Thrown(Value::Object(msg))
            })?;
            let Some(m) = m else { break };
            let abs_start = consumed + m.start;
            let abs_end = consumed + m.end;
            out.extend(&cs[consumed..abs_start]);
            let groups: Vec<Option<(usize, usize)>> = m
                .groups
                .iter()
                .map(|g| g.map(|(a, b)| (consumed + a, consumed + b)))
                .collect();
            out.push_str(&expand_replacement(
                to,
                &cs,
                abs_start,
                abs_end,
                &groups,
                &group_names,
            ));
            consumed = if abs_end == abs_start {
                abs_end + 1
            } else {
                abs_end
            };
            if !global || consumed >= cs.len() {
                break;
            }
        }
        if consumed < cs.len() {
            out.extend(&cs[consumed..]);
        }
        Ok(out)
    }
}

/// 展开替换串中的 `$` 模式（`$&` 全匹配、`` $` `` 前文、`$'` 后文、
/// `$$` 字面 `$`、`$1..$9` 捕获组、`$<name>` 命名捕获组；未参与的组替换为空）。
fn expand_replacement(
    to: &str,
    subject: &[char],
    start: usize,
    end: usize,
    groups: &[Option<(usize, usize)>],
    group_names: &[Option<String>],
) -> String {
    let slice = |a: usize, b: usize| -> String { subject[a..b].iter().collect() };
    let cs: Vec<char> = to.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '$' && i + 1 < cs.len() {
            match cs[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    out.push_str(&slice(start, end));
                    i += 2;
                    continue;
                }
                '`' => {
                    out.push_str(&slice(0, start));
                    i += 2;
                    continue;
                }
                '\'' => {
                    out.push_str(&slice(end, subject.len()));
                    i += 2;
                    continue;
                }
                // `$<name>`：命名捕获组（组未参与或名字不存在 → 空串）
                '<' => {
                    if let Some(close) = cs[i + 2..].iter().position(|&c| c == '>') {
                        let name: String = cs[i + 2..i + 2 + close].iter().collect();
                        let gi = group_names
                            .iter()
                            .position(|n| n.as_deref() == Some(name.as_str()));
                        if let Some(Some((a, b))) = gi.and_then(|n| groups.get(n)) {
                            out.push_str(&slice(*a, *b));
                        }
                        i += 2 + close + 1;
                        continue;
                    }
                }
                d if d.is_ascii_digit() && d != '0' => {
                    let n = d.to_digit(10).unwrap_or(1) as usize - 1;
                    if let Some(Some((a, b))) = groups.get(n) {
                        out.push_str(&slice(*a, *b));
                    }
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// Go rune 字面量形态：普通字符 `'b'`，控制字符用转义序列。
fn quote_rune(b: u8) -> String {
    match b {
        0x0a => "'\\n'".to_owned(),
        0x0d => "'\\r'".to_owned(),
        0x09 => "'\\t'".to_owned(),
        _ => format!("'{}'", b as char),
    }
}

struct JsonParser<'a> {
    b: &'a [u8],
    s: &'a str,
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn parse_value(&mut self, vm: &mut Vm) -> Result<Value, String> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return Err("unexpected end of JSON input".to_owned());
        };
        match c {
            b'{' => self.parse_object(vm),
            b'[' => self.parse_array(vm),
            b'"' => self
                .parse_string()
                .map(|s| Value::Object(vm.alloc_string(s))),
            b't' => self.parse_literal("true", b"rue", Value::Boolean(true)),
            b'f' => self.parse_literal("false", b"alse", Value::Boolean(false)),
            b'n' => self.parse_literal("null", b"ull", Value::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(format!(
                "invalid character {} looking for beginning of value",
                quote_rune(c)
            )),
        }
    }

    fn parse_literal(&mut self, word: &str, rest: &[u8], value: Value) -> Result<Value, String> {
        // 首字符已消费；逐字符校验余下部分（Go 逐字符报错形态）
        for (i, expected) in rest.iter().enumerate() {
            let at = self.pos + 1 + i;
            match self.b.get(at) {
                None => return Err("unexpected end of JSON input".to_owned()),
                Some(&got) if got != *expected => {
                    return Err(format!(
                        "invalid character {} in literal {}",
                        quote_rune(got),
                        word
                    ));
                }
                _ => {}
            }
        }
        self.pos += 1 + rest.len();
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(c) if c.is_ascii_digit() => {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            Some(c) => {
                return Err(format!(
                    "invalid character {} in numeric literal",
                    quote_rune(c)
                ));
            }
            None => return Err("unexpected end of JSON input".to_owned()),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(match self.peek() {
                    Some(c) => format!("invalid character {} in numeric literal", quote_rune(c)),
                    None => "unexpected end of JSON input".to_owned(),
                });
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(match self.peek() {
                    Some(c) => format!("invalid character {} in numeric literal", quote_rune(c)),
                    None => "unexpected end of JSON input".to_owned(),
                });
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = &self.s[start..self.pos];
        text.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| "unexpected end of JSON input".to_owned())
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1; // 开引号
        let mut out = String::new();
        while let Some(c) = self.peek() {
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                0x5c => {
                    self.pos += 1;
                    let Some(esc) = self.peek() else {
                        return Err("unexpected end of JSON input".to_owned());
                    };
                    self.pos += 1;
                    match esc {
                        0x22 => out.push('"'),
                        0x5c => out.push('\\'),
                        0x2f => out.push('/'),
                        0x08 => out.push('\u{8}'),
                        0x0c => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            if self.pos + 4 > self.b.len() {
                                return Err("unexpected end of JSON input".to_owned());
                            }
                            let hex = &self.s[self.pos..self.pos + 4];
                            let cp = u32::from_str_radix(hex, 16)
                                .map_err(|_| "invalid Unicode escape".to_owned())?;
                            self.pos += 4;
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        other => {
                            return Err(format!(
                                "invalid character {} in string escape code",
                                quote_rune(other)
                            ));
                        }
                    }
                }
                c if c < 0x20 => {
                    return Err(format!(
                        "invalid character {} in string literal",
                        quote_rune(c)
                    ));
                }
                _ => {
                    let ch = self.s[self.pos..].chars().next().unwrap_or('\u{FFFD}');
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        Err("unexpected end of JSON input".to_owned())
    }

    fn parse_array(&mut self, vm: &mut Vm) -> Result<Value, String> {
        self.pos += 1; // '['
        let mut elements = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Object(vm.alloc_array(elements)));
        }
        loop {
            let v = self.parse_value(vm)?;
            elements.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Object(vm.alloc_array(elements)));
                }
                Some(c) => {
                    return Err(format!(
                        "invalid character {} after array element",
                        quote_rune(c)
                    ));
                }
                None => return Err("unexpected end of JSON input".to_owned()),
            }
        }
    }

    fn parse_object(&mut self, vm: &mut Vm) -> Result<Value, String> {
        self.pos += 1; // '{'
        let obj = vm.alloc_ordinary();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(obj));
        }
        loop {
            self.skip_ws();
            let Some(key) = self.peek() else {
                return Err("unexpected end of JSON input".to_owned());
            };
            if key != 0x22 {
                return Err(format!(
                    "invalid character {} looking for beginning of object key string",
                    quote_rune(key)
                ));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            match self.peek() {
                Some(b':') => self.pos += 1,
                Some(c) => {
                    return Err(format!(
                        "invalid character {} after object key",
                        quote_rune(c)
                    ));
                }
                None => return Err("unexpected end of JSON input".to_owned()),
            }
            let value = self.parse_value(vm)?;
            let _ = vm.set_property(Value::Object(obj), &key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(obj));
                }
                Some(c) => {
                    return Err(format!(
                        "invalid character {} after object key:value pair",
                        quote_rune(c)
                    ));
                }
                None => return Err("unexpected end of JSON input".to_owned()),
            }
        }
    }
}

/// JSON 字符串引号包裹与转义（对齐 `JSON.stringify` 的字符串形态）。
fn json_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
