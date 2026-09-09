//! package-lock.json v3 读写（npm 9/Node 22 的 lockfileVersion=3 口径）。
//!
//! 保留面：`name` / `version` / `lockfileVersion: 3` / `requires: true` /
//! `packages`（键为 `node_modules/<…>` 路径，含根条目 `""`）。旧版 v1 的
//! `dependencies` 树形态只读兼容降级（忽略，重装时按 v3 重写）——npm 自身
//! 亦在写入时迁移到当前版本。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// lockfile 中单个包条目。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LockPackage {
    /// 包名（根条目用）
    pub name: Option<String>,
    /// 精确解析版本
    pub version: Option<String>,
    /// 解析地址（tarball URL）
    pub resolved: Option<String>,
    /// SRI 完整性
    pub integrity: Option<String>,
    /// 该包的依赖范围声明（原样保留）
    pub dependencies: Option<BTreeMap<String, String>>,
    /// 开发依赖声明（根条目用；普通包条目不含）
    pub dev_dependencies: Option<BTreeMap<String, String>>,
    /// 开发依赖标记
    pub dev: Option<bool>,
    /// 可选依赖标记
    pub optional: Option<bool>,
    /// 对等依赖
    pub peer_dependencies: Option<BTreeMap<String, String>>,
    /// bin 命令面
    pub bin: Option<BTreeMap<String, String>>,
    /// 其余未映射字段（透传）
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// package-lock.json 模型（v3）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lockfile {
    /// 项目名
    pub name: Option<String>,
    /// 项目版本
    pub version: Option<String>,
    /// 固定 3（v3 口径）
    pub lockfile_version: u32,
    /// 根条目 `""`（保留 name/dependencies 声明面）
    #[serde(skip)]
    pub root: LockPackage,
    /// `node_modules/<pkg>` → 条目
    pub packages: BTreeMap<String, LockPackage>,
    /// 其余未映射字段（透传）
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Lockfile {
    /// 新建 v3 空锁文件。
    pub fn new_v3() -> Lockfile {
        Lockfile {
            lockfile_version: 3,
            ..Lockfile::default()
        }
    }

    /// 从文件加载（含 v1/v2 旧形态的最小兼容：取 `packages` 面）。
    pub fn load(path: &Path) -> Result<Lockfile, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
        Lockfile::parse(&text)
    }

    /// 从 JSON 文本解析。
    pub fn parse(text: &str) -> Result<Lockfile, String> {
        let mut lf: Lockfile =
            serde_json::from_str(text).map_err(|e| format!("package-lock.json 解析失败: {e}"))?;
        if lf.lockfile_version == 0 {
            // v1 无 lockfileVersion 字段（缺失时 serde default 0）→ 记为 1
            lf.lockfile_version = 1;
        }
        if let Some(root) = lf.packages.remove("") {
            lf.root = root;
        }
        Ok(lf)
    }

    /// 序列化（npm 风格：2 空格缩进 + 尾换行；根条目写回 `""`）。
    pub fn to_json_string(&self) -> Result<String, String> {
        let mut out = self.clone();
        out.packages.insert(String::new(), out.root.clone());
        let mut s = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        s.push('\n');
        Ok(s)
    }

    /// 登记一个已安装包条目（键 `node_modules/<name>`，嵌套用完整相对路径）。
    pub fn record(&mut self, node_modules_key: &str, pkg: LockPackage) {
        self.packages.insert(node_modules_key.to_owned(), pkg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_roundtrip() {
        let text = r#"{
            "name": "demo",
            "version": "1.0.0",
            "lockfileVersion": 3,
            "requires": true,
            "packages": {
                "": { "name": "demo", "version": "1.0.0", "dependencies": { "is-odd": "^3.0.1" } },
                "node_modules/is-odd": { "version": "3.0.1", "resolved": "https://registry.npmjs.org/is-odd/-/is-odd-3.0.1.tgz", "integrity": "sha512-xxxx", "dependencies": { "is-number": "^6.0.0" } },
                "node_modules/is-number": { "version": "6.0.0" }
            }
        }"#;
        let lf = Lockfile::parse(text).unwrap();
        assert_eq!(lf.lockfile_version, 3);
        assert_eq!(
            lf.root
                .dependencies
                .as_ref()
                .unwrap()
                .get("is-odd")
                .unwrap(),
            "^3.0.1"
        );
        let out = lf.to_json_string().unwrap();
        assert!(out.contains("\"\": {"));
        let lf2 = Lockfile::parse(&out).unwrap();
        assert!(lf2.packages.contains_key("node_modules/is-odd"));
    }

    #[test]
    fn v1_lockfile_is_tolerated() {
        let text = r#"{ "name": "old", "version": "0.1.0", "dependencies": { "a": { "version": "1.0.0" } } }"#;
        let lf = Lockfile::parse(text).unwrap();
        assert_eq!(lf.lockfile_version, 1);
        assert!(lf.packages.is_empty(), "v1 树形态不映射进 v3 packages");
    }
}
