//! package.json 模型解析（npm 语义的 Rust 侧数据面）。
//!
//! 只映射包管理器决策所需的字段；未知字段保留透传（`extra`），写回时
//! 不丢失用户数据。依赖集合统一为「包名 → 范围原文」有序表（npm 的
//! package.json 依赖键序即安装/输出顺序）。

use std::collections::BTreeMap;

use serde::Deserialize;

/// package.json 模型。
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackageJson {
    /// 包名（init 生成 / 安装校验用）
    pub name: Option<String>,
    /// 版本（原文；init 默认 1.0.0）
    pub version: Option<String>,
    /// 描述
    pub description: Option<String>,
    /// 主入口（Node 生态兼容字段）
    pub main: Option<String>,
    /// 脚本表（`npm run` 的键空间）
    pub scripts: BTreeMap<String, String>,
    /// 生产依赖
    pub dependencies: BTreeMap<String, String>,
    /// 开发依赖（install 无参时一并安装；`--production` 跳过）
    pub dev_dependencies: BTreeMap<String, String>,
    /// 对等依赖（解析期登记警告，不自动安装——npm v7+ 默认安装，
    /// 此处按 npm v7+ 语义安装，缺解析时降级警告）
    pub peer_dependencies: BTreeMap<String, String>,
    /// 可选依赖（安装失败不致命）
    pub optional_dependencies: BTreeMap<String, String>,
    /// bin 字段（命令名 → 脚本文件；对象形态与单字符串形态统一展开）
    #[serde(skip)]
    pub bin: BTreeMap<String, String>,
    /// engines（登记不阻断）
    pub engines: BTreeMap<String, String>,
    /// 是否私有（publish 阻断标记；包管理器仅透传）
    pub private: Option<bool>,
    /// workspaces 声明（检测到即警告未支持，不阻断）
    pub workspaces: Option<serde_json::Value>,
    /// 其余未映射字段（写回透传）
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl PackageJson {
    /// 从 JSON 文本解析。
    pub fn parse(text: &str) -> Result<PackageJson, String> {
        let mut pkg: PackageJson =
            serde_json::from_str(text).map_err(|e| format!("package.json 解析失败: {e}"))?;
        // bin 字段双形态归一：字符串 → 以包名为命令名；对象 → 原样
        if let Some(serde_json::Value::Object(map)) = pkg.extra.get("bin") {
            for (k, v) in map {
                if let serde_json::Value::String(s) = v {
                    pkg.bin.insert(k.clone(), s.clone());
                }
            }
        } else if let Some(serde_json::Value::String(s)) = pkg.extra.get("bin") {
            if let Some(name) = &pkg.name {
                pkg.bin.insert(name.clone(), s.clone());
            }
        }
        Ok(pkg)
    }

    /// 从文件路径读取并解析。
    pub fn load(path: &std::path::Path) -> Result<PackageJson, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
        PackageJson::parse(&text)
    }

    /// 序列化为 npm 风格 JSON（2 空格缩进 + 尾换行）。
    pub fn to_json_string(&self) -> Result<String, String> {
        let mut s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        s.push('\n');
        Ok(s)
    }

    /// 生产依赖视图（`--production` / `--omit=dev` 语义）。
    pub fn prod_deps(&self) -> &BTreeMap<String, String> {
        &self.dependencies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_package_json() {
        let pkg = PackageJson::parse(
            r#"{
                "name": "demo",
                "version": "1.2.3",
                "bin": "cli.js",
                "scripts": { "test": "node test.js" },
                "dependencies": { "express": "^4.19.2" },
                "custom-field": { "keep": true }
            }"#,
        )
        .unwrap();
        assert_eq!(pkg.name.as_deref(), Some("demo"));
        assert_eq!(pkg.bin.get("demo").map(String::as_str), Some("cli.js"));
        assert_eq!(
            pkg.scripts.get("test").map(String::as_str),
            Some("node test.js")
        );
        assert_eq!(
            pkg.dependencies.get("express").map(String::as_str),
            Some("^4.19.2")
        );
        // 未知字段透传
        assert!(pkg.extra.contains_key("custom-field"));
    }

    #[test]
    fn bin_object_form_expands() {
        let pkg =
            PackageJson::parse(r#"{ "name": "demo", "bin": { "a": "./a.js", "b": "./b.js" } }"#)
                .unwrap();
        assert_eq!(pkg.bin.len(), 2);
        assert_eq!(pkg.bin.get("a").map(String::as_str), Some("./a.js"));
    }

    #[test]
    fn roundtrip_preserves_unknown_fields() {
        let text = r#"{ "name": "x", "funding": "https://example.com" }"#;
        let pkg = PackageJson::parse(text).unwrap();
        let out = pkg.to_json_string().unwrap();
        assert!(out.contains("funding"));
    }
}
