//! ESM / CJS 模块系统与 Node 解析算法。
//!
//! 承担三件事：把说明符解析成文件路径（含 `package.json` 的 `exports` /
//! `imports` 条件映射）、加载并编译模块、维护实例缓存与循环依赖语义。
//!
//! # 条件解析必须是实例级的
//!
//! `exports` 的条件（`node` / `browser` / `import` / `require`）取决于**谁在
//! 解析**：运行时用 Node 条件，web 打包用 browser 条件。Go 版曾用进程级
//! 全局条件，导致同进程内的 official Vue compiler 与浏览器依赖互相污染，
//! 后来改成 resolver 实例持有条件（`AGENTS.md` 明确禁止回退到全局）。
//! Rust 版从一开始就把条件放在 [`Resolver`] 实例上。

use std::collections::BTreeSet;

/// 解析失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// 找不到模块
    NotFound(String),
    /// `exports` 映射拒绝了该子路径
    ExportsBlocked(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound(spec) => write!(f, "cannot find module '{spec}'"),
            ResolveError::ExportsBlocked(spec) => {
                write!(f, "package exports do not expose '{spec}'")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// 模块的源类型，决定语法与 `this` 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// ES 模块（`import`/`export`，严格模式，顶层 `this` 为 `undefined`）
    EsModule,
    /// CommonJS（`require`/`module.exports`）
    CommonJs,
}

/// 模块说明符解析器。
///
/// 条件集合随实例携带：运行时与打包器各持一个，互不影响。
#[derive(Debug, Clone)]
pub struct Resolver {
    conditions: BTreeSet<String>,
}

impl Resolver {
    /// 以给定条件创建解析器，例如 `["node", "import"]`。
    pub fn new<I, S>(conditions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            conditions: conditions.into_iter().map(Into::into).collect(),
        }
    }

    /// 运行时默认条件集（Node 语义）。
    #[must_use]
    pub fn for_runtime() -> Self {
        Self::new(["node", "import", "default"])
    }

    /// web 打包默认条件集（浏览器语义）。
    #[must_use]
    pub fn for_browser() -> Self {
        Self::new(["browser", "import", "default"])
    }

    /// 该实例是否启用某条件。
    #[must_use]
    pub fn has_condition(&self, name: &str) -> bool {
        self.conditions.contains(name)
    }

    /// 说明符是否为相对路径（`./` 或 `../`）。
    ///
    /// 相对说明符直接按路径解析；裸说明符要走 `node_modules` 查找与
    /// `exports` 映射。
    #[must_use]
    pub fn is_relative(specifier: &str) -> bool {
        specifier.starts_with("./") || specifier.starts_with("../")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_and_browser_resolvers_carry_distinct_conditions() {
        let runtime = Resolver::for_runtime();
        let browser = Resolver::for_browser();

        assert!(runtime.has_condition("node"));
        assert!(!runtime.has_condition("browser"));
        assert!(browser.has_condition("browser"));
        assert!(!browser.has_condition("node"));
    }

    #[test]
    fn relative_specifiers_are_recognised() {
        assert!(Resolver::is_relative("./a.js"));
        assert!(Resolver::is_relative("../b/c.js"));
        assert!(!Resolver::is_relative("express"));
        assert!(!Resolver::is_relative("node:fs"));
    }

    #[test]
    fn module_kinds_are_distinct() {
        assert_ne!(ModuleKind::EsModule, ModuleKind::CommonJs);
    }
}

#[cfg(test)]
mod exports_tests {
    use super::*;

    #[test]
    fn parses_package_json_preserving_key_order() {
        let json = parse_json(r#"{"name":"x","exports":{".":"./a.js","./lib":"./b.js"}}"#)
            .expect("valid json");
        match json.get("exports") {
            Some(Json::Obj(pairs)) => {
                assert_eq!(pairs[0].0, ".");
                assert_eq!(pairs[1].0, "./lib");
            }
            _ => panic!("exports must be object"),
        }
    }

    #[test]
    fn exports_exact_subpath_and_conditions() {
        let exports = parse_json(
            r#"{
              ".": { "import": "./esm/index.js", "require": "./cjs/index.js" },
              "./utils": "./src/utils.js",
              "./private": null
            }"#,
        )
        .expect("valid");
        assert_eq!(
            resolve_exports(&exports, ".", ConditionKind::Import).as_deref(),
            Some("./esm/index.js")
        );
        assert_eq!(
            resolve_exports(&exports, ".", ConditionKind::Require).as_deref(),
            Some("./cjs/index.js")
        );
        assert_eq!(
            resolve_exports(&exports, "./utils", ConditionKind::Require).as_deref(),
            Some("./src/utils.js")
        );
        assert_eq!(
            resolve_exports(&exports, "./private", ConditionKind::Require),
            None
        );
        assert_eq!(
            resolve_exports(&exports, "./nope", ConditionKind::Require),
            None
        );
    }

    #[test]
    fn exports_wildcard_and_nested_conditions() {
        let exports =
            parse_json(r#"{ "./*": { "node": "./dist/*.js" }, "./i18n/*": "./lang/*.json" }"#)
                .expect("valid");
        assert_eq!(
            resolve_exports(&exports, "./core/util", ConditionKind::Require).as_deref(),
            Some("./dist/core/util.js")
        );
        assert_eq!(
            resolve_exports(&exports, "./i18n/zh", ConditionKind::Import).as_deref(),
            Some("./lang/zh.json")
        );
    }

    #[test]
    fn string_exports_only_dot() {
        let exports = parse_json(r#""./main.js""#).expect("valid");
        assert_eq!(
            resolve_exports(&exports, ".", ConditionKind::Require).as_deref(),
            Some("./main.js")
        );
        assert_eq!(
            resolve_exports(&exports, "./x", ConditionKind::Require),
            None
        );
    }

    #[test]
    fn imports_aliases_resolve() {
        let imports = parse_json(
            r##"{ "#internal/*": { "require": "./src/internal/*.js" }, "#config": "./config.js" }"##,
        )
        .expect("valid");
        assert_eq!(
            resolve_imports(&imports, "#internal/util", ConditionKind::Require).as_deref(),
            Some("./src/internal/util.js")
        );
        assert_eq!(
            resolve_imports(&imports, "#config", ConditionKind::Import).as_deref(),
            Some("./config.js")
        );
    }

    #[test]
    fn scoped_specifiers_split() {
        assert_eq!(
            split_package_specifier("@scope/pkg/sub/x"),
            ("@scope/pkg".to_owned(), "./sub/x".to_owned())
        );
        assert_eq!(
            split_package_specifier("lodash"),
            ("lodash".to_owned(), ".".to_owned())
        );
    }
}

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
    if pos != bytes.len() { None } else { Some(v) }
}

fn skip_ws(b: &[char], pos: &mut usize) {
    while *pos < b.len() && matches!(b[*pos], ' ' | '\t' | '\n' | '\r') {
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
                    '\\' => {
                        let esc = b.get(*pos)?;
                        *pos += 1;
                        match esc {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            'b' => out.push('\u{8}'),
                            'f' => out.push('\u{c}'),
                            'u' => {
                                let hex: String = b.get(*pos..*pos + 4)?.iter().collect();
                                *pos += 4;
                                let cp = u32::from_str_radix(&hex, 16).ok()?;
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
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
        Json::Obj(pairs) => resolve_subpath_pairs(pairs, subpath, kind),
        _ => None,
    }
}

/// 子路径映射通用匹配：精确命中优先；其后通配键按 Node
/// PATTERN_KEY_COMPARE 语义取最长字面前缀（更特异者胜）。
fn resolve_subpath_pairs(
    pairs: &[(String, Json)],
    subpath: &str,
    kind: ConditionKind,
) -> Option<String> {
    if let Some((_, target)) = pairs.iter().find(|(k, _)| k == subpath) {
        return resolve_target(target, kind);
    }
    let mut best: Option<(usize, &Json, String)> = None;
    for (k, target) in pairs {
        let Some(star) = k.find('*') else {
            continue;
        };
        let prefix = &k[..star];
        let suffix = &k[star + 1..];
        if subpath.len() >= prefix.len() + suffix.len()
            && subpath.starts_with(prefix)
            && subpath.ends_with(suffix)
        {
            let wild = &subpath[prefix.len()..subpath.len() - suffix.len()];
            let specificity = prefix.len() + suffix.len();
            let better = match &best {
                None => true,
                Some((bs, _, _)) => specificity > *bs,
            };
            if better {
                best = Some((specificity, target, wild.to_owned()));
            }
        }
    }
    let (_, target, wild) = best?;
    resolve_target(target, kind).map(|t| t.replace('*', &wild))
}

/// 解析 `imports` 字段：`alias` 形如 `"#internal/utils"`，返回相对包根目标。
#[must_use]
pub fn resolve_imports(imports: &Json, alias: &str, kind: ConditionKind) -> Option<String> {
    match imports {
        Json::Obj(pairs) => resolve_subpath_pairs(pairs, alias, kind),
        _ => None,
    }
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
