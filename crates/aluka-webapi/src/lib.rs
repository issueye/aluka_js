//! Web API 与 Aluka（Bun 兼容）全局。
//!
//! 这里登记的是**全局能力域**而非模块：`console`、`fetch`、Streams、`Intl`、
//! `crypto.subtle`、`URL`、定时器，以及 `Aluka` 命名空间（`Bun` 为兼容别名）。
//! Go 版把它们按域拆成 13 个子包并要求依赖成 DAG（`internal/runtime/globals/`），
//! Rust 版沿用同样的分域，用 [`Capability`] 枚举把域显式列出来。

/// 一个全局能力域。
///
/// 装配顺序有依赖：例如 `fetch` 依赖 Streams（响应体）与 `URL`（说明符），
/// 所以注册器需要按域而非按单个 API 组织。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// `console`、`navigator`、`BroadcastChannel`
    Console,
    /// `setTimeout` 家族与 `performance`
    Timers,
    /// `TextEncoder`/`TextDecoder`、`atob`/`btoa`
    Encoding,
    /// `ReadableStream`/`WritableStream`/`TransformStream`、`Blob`/`File`、压缩流
    Streams,
    /// `URL`、`URLPattern`
    Url,
    /// `fetch`、`Request`/`Response`/`Headers`/`FormData`、`WebSocket`
    Fetch,
    /// `crypto.subtle`（WebCrypto）
    Crypto,
    /// `Event`、`EventTarget`、`AbortController`、`MessageChannel`
    Events,
    /// `Intl` 全家
    Intl,
    /// `Aluka` 命名空间（`Bun` 兼容别名）
    Aluka,
}

impl Capability {
    /// 该域在装配时依赖的其他域。
    ///
    /// 返回值用于拓扑排序；出现环就是设计错误（Go 版对此立了"依赖必须
    /// 无环"的分包约束）。
    #[must_use]
    pub fn dependencies(self) -> &'static [Capability] {
        match self {
            Capability::Console | Capability::Timers | Capability::Encoding => &[],
            Capability::Events => &[],
            Capability::Url => &[Capability::Encoding],
            Capability::Streams => &[Capability::Events],
            Capability::Crypto => &[Capability::Encoding],
            Capability::Fetch => &[Capability::Streams, Capability::Url, Capability::Events],
            Capability::Intl => &[],
            Capability::Aluka => &[Capability::Fetch, Capability::Streams, Capability::Crypto],
        }
    }

    /// 全部能力域，供装配器遍历。
    #[must_use]
    pub fn all() -> &'static [Capability] {
        &[
            Capability::Console,
            Capability::Timers,
            Capability::Encoding,
            Capability::Events,
            Capability::Url,
            Capability::Streams,
            Capability::Crypto,
            Capability::Fetch,
            Capability::Intl,
            Capability::Aluka,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn dependency_graph_is_acyclic() {
        // 深度优先找回边：能力域之间不允许成环。
        fn visit(node: Capability, path: &mut HashSet<Capability>, done: &mut HashSet<Capability>) {
            if done.contains(&node) {
                return;
            }
            assert!(path.insert(node), "dependency cycle through {node:?}");
            for dep in node.dependencies() {
                visit(*dep, path, done);
            }
            path.remove(&node);
            done.insert(node);
        }

        let mut done = HashSet::new();
        for capability in Capability::all() {
            visit(*capability, &mut HashSet::new(), &mut done);
        }
        assert_eq!(done.len(), Capability::all().len());
    }

    #[test]
    fn fetch_depends_on_streams_and_url() {
        let deps = Capability::Fetch.dependencies();
        assert!(deps.contains(&Capability::Streams));
        assert!(deps.contains(&Capability::Url));
    }

    #[test]
    fn base_capabilities_have_no_dependencies() {
        assert!(Capability::Console.dependencies().is_empty());
        assert!(Capability::Timers.dependencies().is_empty());
    }
}
