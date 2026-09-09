//! registry 历史脏数据宽容反序列化。
//!
//! npm registry 保存着十余年间各种客户端写入的元数据，个别旧版本条目存在
//! 非规范形态（如 `"engines": ">=0.10.40"` 字符串而非对象）。包管理器的
//! 解析必须容忍这些条目（npm 自身宽容），非对象形态一律降级为空映射。

use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

/// 宽容 map 反序列化：JSON 对象 → map；其他形态（字符串/数组/null）→ 空 map。
pub fn lenient_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    let mut out = BTreeMap::new();
    if let Some(serde_json::Value::Object(map)) = raw {
        for (k, v) in map {
            if let serde_json::Value::String(s) = v {
                out.insert(k, s);
            }
        }
    }
    Ok(out)
}
