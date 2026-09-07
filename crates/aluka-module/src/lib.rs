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
