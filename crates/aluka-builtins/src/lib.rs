//! `node:*` 内置模块注册表。
//!
//! 提供 Node.js 22 LTS 标准 59 个核心内置模块的注册表骨架：
//! 模块名 → 模块状态与实现，支持 `require("fs")` 与 `import "node:fs"` 统一解析。
//!
//! # 名称规范
//!
//! 注册表里存**不带前缀**的名字（`fs`、`path`），`node:` 前缀在查询时剥离，
//! 这样 `require("fs")` 与 `import "node:fs"` 命中同一条目。

use std::collections::BTreeMap;

/// 内置模块尚未实现时的占位错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotImplemented {
    /// 模块名
    pub module: String,
}

impl std::fmt::Display for NotImplemented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "builtin module '{}' is not implemented yet", self.module)
    }
}

impl std::error::Error for NotImplemented {}

/// 内置模块注册表。
#[derive(Debug, Default)]
pub struct Registry {
    modules: BTreeMap<String, ModuleStatus>,
}

/// 一个内置模块的实现状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleStatus {
    /// 已有原生实现
    Native,
    /// 外部适配桥接实现
    ForeignBridge,
    /// 仅登记名字，计划实现
    Planned,
}

impl Registry {
    /// 创建空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记全部规划内置模块名，初始状态为 [`ModuleStatus::Planned`]。
    #[must_use]
    pub fn with_planned_modules() -> Self {
        let mut registry = Self::new();
        for name in PLANNED_MODULES {
            registry.register(name, ModuleStatus::Planned);
        }
        registry
    }

    /// 登记（或覆盖）一个模块的状态。
    pub fn register(&mut self, name: &str, status: ModuleStatus) {
        self.modules.insert(name.to_owned(), status);
    }

    /// 查询模块状态；自动剥离 `node:` 前缀。
    #[must_use]
    pub fn status(&self, specifier: &str) -> Option<ModuleStatus> {
        let name = specifier.strip_prefix("node:").unwrap_or(specifier);
        self.modules.get(name).copied()
    }

    /// 已登记的模块数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// 注册表是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// 统计处于某状态的模块数。
    #[must_use]
    pub fn count(&self, status: ModuleStatus) -> usize {
        self.modules
            .values()
            .filter(|current| **current == status)
            .count()
    }
}

/// Node.js 22 LTS 内置模块清单全集。
pub const PLANNED_MODULES: &[&str] = &[
    "path",
    "path/posix",
    "path/win32",
    "os",
    "url",
    "util",
    "util/types",
    "events",
    "diagnostics_channel",
    "async_hooks",
    "fs",
    "fs/promises",
    "assert",
    "assert/strict",
    "constants",
    "crypto",
    "stream",
    "stream/web",
    "stream/promises",
    "stream/consumers",
    "querystring",
    "string_decoder",
    "http",
    "https",
    "net",
    "tls",
    "dns",
    "dns/promises",
    "zlib",
    "perf_hooks",
    "timers",
    "timers/promises",
    "v8",
    "vm",
    "inspector",
    "inspector/promises",
    "dgram",
    "http2",
    "cluster",
    "trace_events",
    "readline",
    "readline/promises",
    "repl",
    "child_process",
    "worker_threads",
    "module",
    "buffer",
    "tty",
    "sqlite",
    "domain",
    "punycode",
    "wasi",
    "process",
    "console",
    "test",
    "test/reporters",
    // node:markdown / aluka:markdown —— Aluka 扩展模块（非 Node 标准）。
    "markdown",
    // node:sys —— node:util 的废弃别名（DEP0140），须与 util 同一对象身份。
    "sys",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_registry_covers_the_go_module_set() {
        let registry = Registry::with_planned_modules();
        assert_eq!(registry.len(), PLANNED_MODULES.len());
        assert_eq!(registry.count(ModuleStatus::Planned), PLANNED_MODULES.len());
        assert_eq!(registry.count(ModuleStatus::Native), 0);
    }

    #[test]
    fn status_lookup_strips_the_node_prefix() {
        let registry = Registry::with_planned_modules();
        assert_eq!(registry.status("fs"), Some(ModuleStatus::Planned));
        assert_eq!(registry.status("node:fs"), Some(ModuleStatus::Planned));
        assert_eq!(
            registry.status("node:fs/promises"),
            Some(ModuleStatus::Planned)
        );
        assert_eq!(registry.status("express"), None);
    }

    #[test]
    fn register_promotes_a_module_through_migration_states() {
        let mut registry = Registry::with_planned_modules();
        registry.register("fs", ModuleStatus::ForeignBridge);
        assert_eq!(registry.status("fs"), Some(ModuleStatus::ForeignBridge));

        registry.register("fs", ModuleStatus::Native);
        assert_eq!(registry.status("fs"), Some(ModuleStatus::Native));
        assert_eq!(registry.count(ModuleStatus::Native), 1);
    }
}
