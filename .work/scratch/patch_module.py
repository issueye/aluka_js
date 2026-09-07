# -*- coding: utf-8 -*-
# aluka-module: 追加 JSON 解析与 exports/imports 条件映射
p = 'crates/aluka-module/src/lib.rs'
s = open(p, encoding='utf-8').read()

addition = '''

/// 极小 JSON 值模型（`package.json` 解析用；保持对象键序——Node 的
/// `exports` 条件匹配依赖键序）。
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// null
    Null,
    /// 布尔
    Bool(bool),
    /// 数字
    Num(f64),
    /// 字符串
    Str(String),
    /// 数组
    Arr(Vec<Json>),
    /// 对象（键按出现序）
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// 取对象字段。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// 以字符串读取。
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// 极小 JSON 解析器（对象键序保持；解析失败返回 `None`）。
#[must_use]
pub fn parse_json(text: &str) -> Option<Json> {
    let bytes: Vec<char> = text.chars().collect();
    let mut pos = 0usize;
    let v = parse_json_value(&bytes, &mut pos)?;
    skip_ws(&bytes, &mut pos);
    if pos != bytes.len() {
        None
    } else {
        Some(v)
    }
}

fn skip_ws(b: &[char], pos: &mut usize) {
    while *pos < b.len() && matches!(b[*pos], ' ' | '\\t' | '\\n' | '\\r') {
        *pos += 1;
    }
}

fn parse_json_value(b: &[char], pos: &mut usize) -> Option<Json> {
    skip_ws(b, pos);
    match b.get(*pos)? {
        '{' => {
            *pos += 1;
            let mut pairs = Vec::new();
            skip_ws(b, pos);
            if b.get(*pos) == Some(&'}') {
                *pos += 1;
                return Some(Json::Obj(pairs));
            }
            loop {
                skip_ws(b, pos);
                let key = match parse_json_value(b, pos)? {
                    Json::Str(s) => s,
                    _ => return None,
                };
                skip_ws(b, pos);
                if b.get(*pos) != Some(&':') {
                    return None;
                }
                *pos += 1;
                let value = parse_json_value(b, pos)?;
                pairs.push((key, value));
                skip_ws(b, pos);
                match b.get(*pos) {
                    Some(',') => *pos += 1,
                    Some('}') => {
                        *pos += 1;
                        return Some(Json::Obj(pairs));
                    }
                    _ => return None,
                }
            }
        }
        '[' => {
            *pos += 1;
            let mut items = Vec::new();
            skip_ws(b, pos);
            if b.get(*pos) == Some(&']') {
                *pos += 1;
                return Some(Json::Arr(items));
            }
            loop {
                let v = parse_json_value(b, pos)?;
                items.push(v);
                skip_ws(b, pos);
                match b.get(*pos) {
                    Some(',') => *pos += 1,
                    Some(']') => {
                        *pos += 1;
                        return Some(Json::Arr(items));
                    }
                    _ => return None,
                }
            }
        }
        '"' => {
            *pos += 1;
            let mut out = String::new();
            loop {
                let c = b.get(*pos)?;
                *pos += 1;
                match c {
                    '"' => return Some(Json::Str(out)),
                    '\\\\' => {
                        let esc = b.get(*pos)?;
                        *pos += 1;
                        match esc {
                            'n' => out.push('\\n'),
                            't' => out.push('\\t'),
                            'r' => out.push('\\r'),
                            'b' => out.push('\\u{8}'),
                            'f' => out.push('\\u{c}'),
                            'u' => {
                                let hex: String = b.get(*pos..*pos + 4)?.iter().collect();
                                *pos += 4;
                                let cp = u32::from_str_radix(&hex, 16).ok()?;
                                out.push(char::from_u32(cp).unwrap_or('\\u{FFFD}'));
                            }
                            other => out.push(*other),
                        }
                    }
                    other => out.push(*other),
                }
            }
        }
        't' => {
            if b.get(*pos..*pos + 4) == Some(&['t', 'r', 'u', 'e'][..]) {
                *pos += 4;
                Some(Json::Bool(true))
            } else {
                None
            }
        }
        'f' => {
            if b.get(*pos..*pos + 5) == Some(&['f', 'a', 'l', 's', 'e'][..]) {
                *pos += 5;
                Some(Json::Bool(false))
            } else {
                None
            }
        }
        'n' => {
            if b.get(*pos..*pos + 4) == Some(&['n', 'u', 'l', 'l'][..]) {
                *pos += 4;
                Some(Json::Null)
            } else {
                None
            }
        }
        c if *c == '-' || c.is_ascii_digit() => {
            let start = *pos;
            if b.get(*pos) == Some(&'-') {
                *pos += 1;
            }
            while b
                .get(*pos)
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                *pos += 1;
            }
            let text: String = b[start..*pos].iter().collect();
            text.parse::<f64>().ok().map(Json::Num)
        }
        _ => None,
    }
}

/// 拆分裸说明符为（包名, 子路径）：`@scope/pkg/x` → ("@scope/pkg", "./x")；
/// `pkg/sub/y` → ("pkg", "./sub/y")；`pkg` → ("pkg", ".")。
#[must_use]
pub fn split_package_specifier(specifier: &str) -> (String, String) {
    let (name, rest) = if let Some(stripped) = specifier.strip_prefix('@') {
        match stripped.find('/') {
            Some(i) => match stripped[i + 1..].find('/') {
                Some(j) => (&specifier[..i + 1 + j + 1], &specifier[i + 1 + j + 2..]),
                None => (specifier, ""),
            },
            None => (specifier, ""),
        }
    } else {
        match specifier.find('/') {
            Some(i) => (&specifier[..i], &specifier[i + 1..]),
            None => (specifier, ""),
        }
    };
    let sub = if rest.is_empty() {
        ".".to_owned()
    } else {
        format!("./{rest}")
    };
    (name.to_owned(), sub)
}

/// 条件集合（require / import 两类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionKind {
    /// CJS require 语义：`["node", "require", "default"]`
    Require,
    /// ESM import 语义：`["node", "import", "default"]`
    Import,
}

impl ConditionKind {
    /// 活跃条件集合。
    #[must_use]
    pub fn active(self) -> &'static [&'static str] {
        match self {
            Self::Require => &["node", "require", "default"],
            Self::Import => &["node", "import", "default"],
        }
    }
}

/// 解析 `exports` 字段：`subpath` 形如 `"."` 或 `"./lib/x"`，返回相对包根的
/// 目标路径（如 `"./dist/index.js"`）。遵循 Node 条件匹配（保持对象键序、
/// 先到先得）、`null` 阻断与 `./*` 通配。
#[must_use]
pub fn resolve_exports(exports: &Json, subpath: &str, kind: ConditionKind) -> Option<String> {
    match exports {
        Json::Str(s) => {
            if subpath == "." {
                normalize_target(s)
            } else {
                None
            }
        }
        Json::Obj(pairs) => {
            if let Some((_, target)) = pairs.iter().find(|(k, _)| k == subpath) {
                return resolve_target(target, kind);
            }
            for (k, target) in pairs {
                if let Some(stripped) = k.strip_prefix("./") {
                    if let Some(star) = stripped.find('*') {
                        let prefix = &stripped[..star];
                        let suffix = &stripped[star + 1..];
                        let sub = subpath.strip_prefix("./").unwrap_or(subpath);
                        if sub.len() >= prefix.len() + suffix.len()
                            && sub.starts_with(prefix)
                            && sub.ends_with(suffix)
                        {
                            let wild = &sub[prefix.len()..sub.len() - suffix.len()];
                            return resolve_target(target, kind).map(|t| t.replace('*', wild));
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// 解析 `imports` 字段：`alias` 形如 `"#internal/utils"`，返回相对包根目标。
#[must_use]
pub fn resolve_imports(imports: &Json, alias: &str, kind: ConditionKind) -> Option<String> {
    let pairs = match imports {
        Json::Obj(pairs) => pairs,
        _ => return None,
    };
    let target = pairs.iter().find(|(k, _)| k == alias).map(|(_, v)| v)?;
    resolve_target(target, kind)
}

/// 解析目标值：字符串 / 条件对象 / 数组回退 / null 阻断。
fn resolve_target(target: &Json, kind: ConditionKind) -> Option<String> {
    match target {
        Json::Null => None,
        Json::Str(s) => normalize_target(s),
        Json::Arr(items) => items.iter().find_map(|t| resolve_target(t, kind)),
        Json::Obj(pairs) => {
            for (cond, nested) in pairs {
                if kind.active().contains(&cond.as_str()) {
                    if let Some(t) = resolve_target(nested, kind) {
                        return Some(t);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// 目标路径规范化：必须以 `./` 起始（Node 规范）。
fn normalize_target(s: &str) -> Option<String> {
    if s.starts_with("./") {
        Some(s.to_owned())
    } else {
        None
    }
}
'''

s += addition
open(p, 'w', encoding='utf-8').write(s)
print("appended", len(addition), "chars")
