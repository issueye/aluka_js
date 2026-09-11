//! 内建原语补齐：`JSON.stringify`、字符串原型方法、`String`/`Symbol` 全局函数。
//!
//! 语义对齐 Node.js 22 LTS 规范：
//! - `JSON.stringify(undefined)` 返回 `undefined`；函数/符号同（顶层）；
//! - 对象键序 = 整数索引键升序前置 + 其余按**创建序**（`Ordinary` 快速与
//!   字典模式均保插入序，对齐 V8 键序）；
//! - 对象属性值为 `undefined`/函数/符号 → 整键剔除；数组元素同值 → `"null"`；
//! - 字符串方法直接在 `CALL_METHOD` 链求值，不物化原型方法占位。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

/// 对象属性值是否为 JSON 忽略值（整键剔除）：`undefined` / 函数 / 符号。
fn is_json_ignored_value(vm: &Vm, v: Value) -> bool {
    match v.case() {
        ValueCase::Undefined => true,
        ValueCase::Object(r) => matches!(
            vm.heap.get(r.0 as usize),
            Some(
                HeapObject::Closure { .. }
                    | HeapObject::NativeFn { .. }
                    | HeapObject::NativeCtor { .. }
                    | HeapObject::Symbol { .. }
            )
        ),
        _ => false,
    }
}

impl Vm {
    /// 判断值是否为 JSON 全局对象（`_isJSON` 标记）。
    pub(crate) fn is_json_object(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r) if self.has_own_slot(r.0 as usize, "_isJSON")
        )
    }

    /// `JSON.stringify(value[, replacer[, space]])`（replacer/space 忽略）。
    ///
    /// 顶层 undefined / 函数 / 符号 → 返回 `undefined`（标准语义）；其余
    /// 值序列化为字符串。
    pub(crate) fn json_stringify(&mut self, value: Value) -> Result<Value, VmError> {
        // 规范 `SerializeJSONProperty`：根值等价于以键 `""` 序列化，故 `toJSON`
        // 先于「不可序列化」判定生效（`JSON.stringify({d:new Date(0)})` 的 ISO 串
        // 形态即由 `Date.prototype.toJSON` 产出）。
        let value = self.apply_to_json(value, "")?;
        // 顶层不可序列化值（含 toJSON 返回的 undefined）：undefined / 函数 / 符号
        if matches!(value, Value::Undefined) || is_json_ignored_value(self, value) {
            return Ok(Value::Undefined);
        }
        if let Some(r) = value.as_object() {
            // Promise 等无自有可枚举属性的异形堆对象：node 序列化为 "{}"
            // 而非 null（`JSON.stringify(Promise.resolve(1))` 实测）
            if matches!(
                self.heap.get(r.0 as usize),
                Some(HeapObject::Promise { .. })
            ) {
                let s = self.alloc_string("{}".to_owned());
                return Ok(Value::Object(s));
            }
        }
        let mut out = String::new();
        self.json_write(&mut out, value, &mut Vec::new())?;
        Ok(Value::Object(self.alloc_string(out)))
    }

    /// 规范 `SerializeJSONProperty` 第 2 步：值为对象且其 `toJSON` **可调用**时，
    /// 以属性键为唯一实参调用，并用返回值继续序列化；否则原值返回。
    ///
    /// 键规则（规范）：根值为 `""`、对象属性为属性名、数组元素为下标字符串——
    /// 故 `JSON.stringify({a:{toJSON(k){return k}}})` 得 `{"a":"a"}`；
    /// 而 `Date.prototype.toJSON` 即经此命中（`this` 为 Date 实例）。
    /// `toJSON` 经**原型链**查找（`get_property`），与规范 `GetV` 一致。
    fn apply_to_json(&mut self, value: Value, key: &str) -> Result<Value, VmError> {
        // 仅对象（含函数、数组）参与；原始值直接返回
        if !matches!(value.case(), ValueCase::Object(_)) {
            return Ok(value);
        }
        let cb = self.get_property(value, "toJSON")?;
        let callable = match cb.case() {
            ValueCase::Object(cr) => {
                // Proxy 视为可调用（`invoke_callable` 会走 apply trap）；
                // 其余按堆变体判定函数面
                self.proxy_parts(cr).is_some()
                    || matches!(
                        self.heap.get(cr.0 as usize),
                        Some(
                            HeapObject::Closure { .. }
                                | HeapObject::NativeFn { .. }
                                | HeapObject::NativeCtor { .. }
                        )
                    )
            }
            _ => false,
        };
        if !callable {
            return Ok(value);
        }
        let key_arg = Value::Object(self.alloc_string(key.to_owned()));
        self.invoke_callable(cb, value, &[key_arg])
    }

    /// 判断字符串是否为 JSON 数组索引键（规范 [[OwnPropertyKeys]] 整数键前置）。
    fn is_json_array_index(key: &str) -> bool {
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        // 无前导零（"0" 除外）
        if key.len() > 1 && key.starts_with('0') {
            return false;
        }
        key.parse::<u64>().map(|n| n <= 4294967294).unwrap_or(false)
    }

    /// 递归序列化。`seen` 持有栈上对象句柄做循环引用检测（循环 → `"null"`，
    /// 对齐标准 `TypeError` 之外的常见降级；Go 侧实测无循环用例）。
    ///
    /// 语义对齐 Node.js 22 LTS：
    /// - `toJSON` 协议（规范 `SerializeJSONProperty`）：由**调用方**在取用处按
    ///   属性键应用一次（根为 `""`、对象属性为属性名、数组元素为下标串），
    ///   故本函数自身不再重复应用——避免 `toJSON` 被调用两次的可见副作用；
    /// - 对象键序 = 整数索引键升序前置 + 其余**创建序**（shape/dict 保插入序）；
    /// - 对象属性值为 `undefined`/函数/符号 → 整键剔除（标准 SerializeJSONObject）；
    /// - 数组元素 `undefined`/函数 → `"null"` 占位（标准）；
    /// - 顶层 `undefined` 返回 `undefined`（见 [`json_stringify`]）。
    ///
    /// 签名为 `&mut self`：`toJSON` 可能是用户函数，调用它需要可变借用。故各分支
    /// **先把堆变体快照为 owned 数据**再递归——不可持有 `self.heap` 借用跨调用。
    fn json_write(
        &mut self,
        out: &mut String,
        value: Value,
        seen: &mut Vec<u32>,
    ) -> Result<(), VmError> {
        match value.case() {
            ValueCase::Undefined | ValueCase::Null => out.push_str("null"),
            ValueCase::Boolean(b) => out.push_str(if b { "true" } else { "false" }),
            ValueCase::Number(n) => {
                if n.is_nan() || n.is_infinite() {
                    out.push_str("null");
                } else {
                    out.push_str(&crate::ops::js_number_to_string(n));
                }
            }
            ValueCase::Object(r) => {
                if seen.contains(&r.0) {
                    out.push_str("null");
                    return Ok(());
                }
                // 堆变体快照（owned；借用在此结束，后续递归可安全取 &mut self）
                enum Kind {
                    Text(String),
                    Arr(usize),
                    Obj(Vec<(String, Value)>),
                    Other,
                }
                let kind = match self.heap.get(r.0 as usize) {
                    Some(HeapObject::String(text)) => Kind::Text(text.clone()),
                    Some(HeapObject::Array { elements, .. }) => Kind::Arr(elements.len()),
                    Some(HeapObject::Ordinary { .. } | HeapObject::Map { .. }) => {
                        Kind::Obj(self.own_entries(r.0 as usize))
                    }
                    _ => Kind::Other,
                };
                match kind {
                    Kind::Text(text) => out.push_str(&json_quote(&text)),
                    Kind::Arr(len) => {
                        seen.push(r.0);
                        out.push('[');
                        for i in 0..len {
                            if i > 0 {
                                out.push(',');
                            }
                            // 按下标逐次读取（不持有 elements 借用，见函数文档）
                            let el = match self.heap.get(r.0 as usize) {
                                Some(HeapObject::Array { elements, .. }) => {
                                    elements.get(i).copied().unwrap_or(Value::Undefined)
                                }
                                _ => Value::Undefined,
                            };
                            // 规范：数组元素的 toJSON 以**下标字符串**为键
                            let ek = i.to_string();
                            let el = self.apply_to_json(el, &ek)?;
                            // 数组内的 undefined/函数/符号均序列化为 "null"（标准）
                            if is_json_ignored_value(self, el) {
                                out.push_str("null");
                            } else {
                                self.json_write(out, el, seen)?;
                            }
                        }
                        out.push(']');
                        seen.pop();
                    }
                    // Map/Set 也在此分支：其内部条目不是属性，`own_entries` 对
                    // `HeapObject::Map` 返回空 → 序列化为 `{}`，与 Node 一致
                    // （`JSON.stringify(new Map())` 曾落入 `_ => "null"` 得到 `null`）。
                    // 登记：用户额外挂在 Map 上的自有属性同样不会被序列化（`own_entries`
                    // 只读 Ordinary 的 props）——属本实现折衷，未在 Node 中复现。
                    Kind::Obj(entries) => {
                        seen.push(r.0);
                        out.push('{');
                        // 键序（规范 [[OwnPropertyKeys]] 的 JSON 子集）：
                        // 整数索引键按数值升序前置，其余键保持创建序（VM 的
                        // shape/字典两模式均保插入序）。符号键不参与（标准）。
                        // 稳定分区：整数键（已升序收集）前置，非整数键保创建序
                        let mut idx_items: Vec<(String, Value)> = Vec::new();
                        let mut str_items: Vec<(String, Value)> = Vec::new();
                        for (k, v) in entries {
                            if crate::symbol::is_symbol_key(&k) {
                                continue;
                            }
                            // 规范：对象属性的 toJSON 以**属性名**为键，且先于
                            // 「不可序列化则剔除」判定（返回值可能变为可序列化）
                            let v = self.apply_to_json(v, &k)?;
                            if is_json_ignored_value(self, v) {
                                continue;
                            }
                            if Self::is_json_array_index(&k) {
                                idx_items.push((k, v));
                            } else {
                                str_items.push((k, v));
                            }
                        }
                        // 整数键字典序 == 数值序（无前导零的十进制串）
                        idx_items.sort_by(|a, b| a.0.cmp(&b.0));
                        idx_items.extend(str_items);
                        for (i, (k, v)) in idx_items.iter().enumerate() {
                            if i > 0 {
                                out.push(',');
                            }
                            out.push_str(&json_quote(k));
                            out.push(':');
                            self.json_write(out, *v, seen)?;
                        }
                        out.push('}');
                        seen.pop();
                    }
                    Kind::Other => out.push_str("null"),
                }
            }
        }
        Ok(())
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
        #[allow(non_snake_case)]
        fn Number(n: f64) -> Value {
            Value::Number(n)
        }
        // 借用隔离：arg_str/arg_num 提为自由函数（self 顺序借用）
        fn arg_str(vm: &mut Vm, args: &[Value], i: usize) -> String {
            args.get(i).map(|v| vm.format_value(*v)).unwrap_or_default()
        }
        fn arg_num(args: &[Value], i: usize) -> Option<f64> {
            args.get(i).and_then(|v| match v.case() {
                ValueCase::Number(n) => Some(n),
                _ => None,
            })
        }
        // 索引类参数按 JS ToInteger 语义强转：数字直用，字符串解析数值
        //（如 `charCodeAt('1')` → 1，对齐 Node），其余非数字为 NaN
        fn arg_index_num(vm: &Vm, args: &[Value], i: usize) -> f64 {
            match args.get(i).map(|v| v.case()).unwrap_or(ValueCase::Undefined) {
                Some(ValueCase::Number(n)) => n,
                Some(ValueCase::Object(r)) => match vm.heap.get(r.0 as usize) {
                    Some(HeapObject::String(s)) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
                    _ => f64::NAN,
                },
                Some(ValueCase::Boolean(true)) => 1.0,
                Some(ValueCase::Boolean(false)) | Some(Value::Null) => 0.0,
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
            "codePointAt" => {
                // `codePointAt(pos)`：返回该位置的**码点**（越界 → undefined）。
                // 注：本实现的字符串按码点寻址（Rust String 为合法 UTF-8，无法表示
                // 孤立代理），故对代理对的**低位下标**与 Node 不同（Node 返回低位代理
                // 值 0xDC00..0xDFFF）——已登记偏离，仅影响显式寻址到代理对内部的用法。
                let i = arg_index_num(self, args, 0);
                // 规范 ToIntegerOrInfinity：NaN → 0（`'a'.codePointAt(NaN)` → 97）
                let i = if i.is_nan() { 0.0 } else { i.trunc() };
                let cp = if i < 0.0 {
                    None
                } else {
                    chars.get(i as usize).map(|c| *c as u32 as f64)
                };
                Some(Ok(cp.map(Number).unwrap_or(Value::Undefined)))
            }
            "at" => {
                // `at(i)`：负下标自尾部计数；越界 → undefined（Node 语义）
                // 规范 ToIntegerOrInfinity：NaN → 0（`'abc'.at(NaN)` → "a"）
                let n = arg_index_num(self, args, 0);
                let n = if n.is_nan() { 0.0 } else { n.trunc() };
                let len = chars.len() as f64;
                let idx = if n < 0.0 { len + n } else { n };
                let out = if idx < 0.0 || idx >= len {
                    Value::Undefined
                } else {
                    Value::Object(self.alloc_string(chars[idx as usize].to_string()))
                };
                Some(Ok(out))
            }
            "padStart" | "padEnd" => {
                // `padStart(targetLength[, padString])`：不足则用 padString **循环
                // 截断**补齐（默认空格）；已足够或 padString 为空 → 原串返回。
                let target_f = arg_num(args, 0).unwrap_or(0.0);
                let target = if target_f.is_nan() || target_f < 0.0 {
                    0usize
                } else {
                    target_f as usize
                };
                let fill = match args.get(1) {
                    Some(v) if !matches!(*v, Value::Undefined) => self.format_value(*v),
                    _ => " ".to_owned(),
                };
                if target <= chars.len() || fill.is_empty() {
                    ret_str!(text.to_owned());
                }
                let pad_len = target - chars.len();
                let fill_chars: Vec<char> = fill.chars().collect();
                let mut pad = String::with_capacity(pad_len);
                for k in 0..pad_len {
                    pad.push(fill_chars[k % fill_chars.len()]);
                }
                let out = if method == "padStart" {
                    format!("{pad}{text}")
                } else {
                    format!("{text}{pad}")
                };
                ret_str!(out);
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
                // RegExp 实参：正则替换（`g` 标志替换全部，否则首个）；
                // 第二实参为函数时按 replacer 回调语义（get-intrinsic 的
                // stringToPath 用 `$replace(str, rePropName, fn)` 形态）
                if let Some(re) = args.first().copied() {
                    if self.is_regexp_obj(re) {
                        let replacer = replacer_of(self, args, 1);
                        match self.regexp_replace(re, text, &replacer, false) {
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
                        let replacer = replacer_of(self, args, 1);
                        match self.regexp_replace(re, text, &replacer, true) {
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
                        let limit = args.get(1).and_then(|v| match v.case() {
                            ValueCase::Number(n) if n >= 0.0 => Some(n as usize),
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
        to: &Replacer,
        replace_all: bool,
    ) -> Result<String, VmError> {
        let (pattern, flags) = match re.case() {
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
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
            match to {
                Replacer::Str(rep) => out.push_str(&expand_replacement(
                    rep,
                    &cs,
                    abs_start,
                    abs_end,
                    &groups,
                    &group_names,
                )),
                Replacer::Fn(cb) => {
                    // 回调参数：match 全串、捕获组（未参与 undefined）、offset、subject
                    let mut cb_args: Vec<Value> = Vec::with_capacity(groups.len() + 3);
                    let full: String = cs[abs_start..abs_end].iter().collect();
                    cb_args.push(Value::Object(self.alloc_string(full)));
                    for g in &groups {
                        match g {
                            Some((a, b)) => {
                                let txt: String = cs[*a..*b].iter().collect();
                                cb_args.push(Value::Object(self.alloc_string(txt)));
                            }
                            None => cb_args.push(Value::Undefined),
                        }
                    }
                    cb_args.push(Value::Number(abs_start as f64));
                    let subject = self.alloc_string(text.to_owned());
                    cb_args.push(Value::Object(subject));
                    let ret = self.invoke_callable(*cb, Value::Undefined, &cb_args)?;
                    out.push_str(&self.format_value(ret));
                }
            }
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

/// `replace` 的替换物：静态串或 replacer 函数（回调模式）。
pub(crate) enum Replacer {
    Str(String),
    Fn(Value),
}

/// 从调用实参构造替换物（函数实参 → 回调模式；其余按字符串格式化）。
fn replacer_of(vm: &mut Vm, args: &[Value], i: usize) -> Replacer {
    match args.get(i).copied() {
        Some(v) if vm.resolve_callable(v).0.is_some() => Replacer::Fn(v),
        Some(v) => Replacer::Str(vm.format_value(v)),
        None => Replacer::Str("undefined".to_owned()),
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
