//! registry 拉取与版本解析：packument（包元数据全集）→ 满足范围的最高版本。
//!
//! npm 解析语义要点：
//! - 候选集 = packument `versions` 键全集（含预发布）；
//! - 范围匹配按 [`crate::semver`] 的 satisfies（预发布同元组守卫）；
//! - 无范围命中时的 dist-tag 兜底：spec 带 tag（`express@latest`）按 tag 解析；
//!   `foo@`（空范围）= `*`。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::http::{HttpClient, HttpError};
use crate::semver::{Range, Version};

/// 单版本元数据（packument `versions` 条目中本包管理器关心的面）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionMeta {
    /// 包名
    pub name: String,
    /// 精确版本号
    pub version: String,
    /// tarball 下载地址
    pub dist: Dist,
    /// 该版本的依赖
    #[serde(deserialize_with = "crate::lenient::lenient_map")]
    pub dependencies: BTreeMap<String, String>,
    /// 该版本的对等依赖（npm v7+ 会随安装解析）
    #[serde(deserialize_with = "crate::lenient::lenient_map")]
    pub peer_dependencies: BTreeMap<String, String>,
    /// 该版本的可选依赖
    #[serde(deserialize_with = "crate::lenient::lenient_map")]
    pub optional_dependencies: BTreeMap<String, String>,
    /// 开发依赖（不安装，仅透传展示）
    #[serde(deserialize_with = "crate::lenient::lenient_map")]
    pub dev_dependencies: BTreeMap<String, String>,
    /// bin 命令面（对象/字符串归一后）
    #[serde(skip)]
    pub bin: BTreeMap<String, String>,
    /// 是否已弃用（`deprecated` 文本存在即标记）
    pub deprecated: Option<String>,
    /// engines 要求（历史条目可能为字符串 → 宽容降级空映射）
    #[serde(deserialize_with = "crate::lenient::lenient_map")]
    pub engines: BTreeMap<String, String>,
    /// 其余未映射字段
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// 发行信息（tarball 地址 + SRI 完整性）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Dist {
    /// tarball URL
    pub tarball: String,
    /// SRI 完整性（`sha512-<base64>`）
    pub integrity: Option<String>,
    /// 旧式 shasum（hex sha1；integrity 缺失时降级校验不可用则仅告警）
    pub shasum: Option<String>,
}

/// packument：包名下的全部版本与 dist-tags。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Packument {
    /// 包名
    pub name: String,
    /// 精确版本号 → 版本元数据
    pub versions: BTreeMap<String, VersionMeta>,
    /// dist-tags（`latest` 等）
    #[serde(rename = "dist-tags")]
    pub dist_tags: BTreeMap<String, String>,
    /// 其余未映射字段
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Packument {
    /// 拉取 packument（registry 默认 `https://registry.npmjs.org`）。
    pub fn fetch(client: &HttpClient, registry: &str, name: &str) -> Result<Packument, HttpError> {
        let base = registry.trim_end_matches('/');
        let encoded = encode_pkg_name(name);
        let url = format!("{base}/{encoded}");
        client.get_json(&url).and_then(|v| {
            serde_json::from_value(v)
                .map_err(|e| HttpError::Body(format!("packument 解析失败: {e}")))
        })
    }

    /// 全部候选版本（解析失败或非法版本号跳过）。
    pub fn candidates(&self) -> Vec<Version> {
        self.versions
            .keys()
            .filter_map(|s| Version::parse(s).ok())
            .collect()
    }

    /// 在候选中选出满足范围的最佳版本（版本降序最高）。
    ///
    /// 返回 `(版本原文, 元数据)`；范围无命中返回 `None`。
    pub fn resolve(&self, range: &Range) -> Option<(String, &VersionMeta)> {
        let mut best: Option<(&String, &VersionMeta, Version)> = None;
        for (vstr, meta) in &self.versions {
            let Ok(v) = Version::parse(vstr) else {
                continue;
            };
            if !range.satisfies(&v) {
                continue;
            }
            let better = match &best {
                None => true,
                Some((_, _, bv)) => v > *bv,
            };
            if better {
                best = Some((vstr, meta, v));
            }
        }
        best.map(|(s, m, _)| (s.clone(), m))
    }
}

/// 包名 URL 编码（scoped 包 `@scope/name` → `@scope%2fname`；registry 口径）。
fn encode_pkg_name(name: &str) -> String {
    if let Some((scope, rest)) = name.strip_prefix('@').and_then(|s| s.split_once('/')) {
        format!("@{scope}%2f{rest}")
    } else {
        name.to_owned()
    }
}

/// 解析安装 spec（`name` / `name@range` / `@scope/name@range`）。
///
/// 返回 `(包名, 范围)`。spec 无 `@范围` 后缀时范围 = `*`（npm 默认 latest
/// 标签语义经范围 `*` + 最高稳定版等价达成）。scoped 名的首个 `@` 不算分隔符。
pub fn parse_spec(spec: &str) -> Result<(String, Range), String> {
    let (name, range_str) = if let Some(rest) = spec.strip_prefix('@') {
        // scoped：找第二个 `/` 之后的 `@`
        match rest.split_once('/') {
            None => (format!("@{rest}"), ""),
            Some((scope, tail)) => match tail.rfind('@') {
                Some(pos) => (format!("@{scope}/{}", &tail[..pos]), &tail[pos + 1..]),
                None => (format!("@{scope}/{tail}"), ""),
            },
        }
    } else {
        match spec.find('@') {
            Some(pos) => (spec[..pos].to_owned(), &spec[pos + 1..]),
            None => (spec.to_owned(), ""),
        }
    };
    if name.is_empty() {
        return Err(format!("空包名: {spec}"));
    }
    let range_str = range_str.trim();
    let range = if range_str.is_empty() {
        Range::parse("*")?
    } else {
        Range::parse(range_str)?
    };
    Ok((name, range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parsing_covers_npm_forms() {
        let (n, r) = parse_spec("express").unwrap();
        assert_eq!(n, "express");
        assert!(r.satisfies(&Version::parse("99.0.0").unwrap()));
        let (n, r) = parse_spec("express@^4.19.2").unwrap();
        assert_eq!(n, "express");
        assert!(r.satisfies(&Version::parse("4.19.2").unwrap()));
        assert!(!r.satisfies(&Version::parse("5.0.0").unwrap()));
        let (n, r) = parse_spec("@babel/core@7.24.0").unwrap();
        assert_eq!(n, "@babel/core");
        assert!(r.satisfies(&Version::parse("7.24.0").unwrap()));
        assert!(!r.satisfies(&Version::parse("7.25.0").unwrap()));
        let (n, _) = parse_spec("@babel/core").unwrap();
        assert_eq!(n, "@babel/core");
        let (n, r) = parse_spec("lodash@4").unwrap();
        assert_eq!(n, "lodash");
        assert!(r.satisfies(&Version::parse("4.17.21").unwrap()));
        assert!(!r.satisfies(&Version::parse("3.9.3").unwrap()));
    }

    #[test]
    fn scoped_name_url_encoding() {
        assert_eq!(encode_pkg_name("express"), "express");
        assert_eq!(encode_pkg_name("@babel/core"), "@babel%2fcore");
    }
}
