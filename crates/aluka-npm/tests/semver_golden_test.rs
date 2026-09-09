//! semver 引擎黄金对拍：与真实 `semver` npm 包（node-semver 7.7.3，
//! Node.js 22 生态权威实现）的 `satisfies` 逐对全量比对。
//!
//! 语料 `semver_golden.json` 由 `gen.mjs` 在真实包上一次性生成（27 版本
//! × 79 范围，覆盖精确/等号/v 前缀/x-range/波浪/插入/原语/连字符/并集/
//! AND 复合/预发布守卫/空白容忍全语法面），任何引擎行为偏差都会在此
//! 全量暴露。

use aluka_npm::semver::{Range, Version};

#[derive(serde::Deserialize)]
struct Golden {
    versions: Vec<String>,
    rows: Vec<GoldenRow>,
}

#[derive(serde::Deserialize)]
struct GoldenRow {
    range: String,
    /// 真实 semver 包判定的满足版本全集
    #[serde(rename = "match")]
    match_: Vec<String>,
}

fn load_golden() -> Golden {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/semver_golden.json"
    ))
    .expect("黄金语料存在");
    serde_json::from_str(&text).expect("黄金语料可解析")
}

#[test]
fn satisfies_matches_real_semver_package_exactly() {
    let golden = load_golden();
    let versions: Vec<Version> = golden
        .versions
        .iter()
        .map(|v| Version::parse(v).expect("语料版本合法"))
        .collect();
    let mut mismatches: Vec<String> = Vec::new();
    for row in &golden.rows {
        let Ok(range) = Range::parse(&row.range) else {
            mismatches.push(format!("range 解析失败: {:?}", row.range));
            continue;
        };
        for (vtext, v) in golden.versions.iter().zip(&versions) {
            let expect = row.match_.contains(vtext);
            let got = range.satisfies(v);
            if got != expect {
                mismatches.push(format!(
                    "range {:?} × version {}: 引擎={} semver包={}",
                    row.range, vtext, got, expect
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "与真实 semver 包存在 {} 处分歧:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn version_ordering_matches_semver_compare() {
    // node-semver 官方 README 排序断言（含预发布数值/字典序混合规则）
    let asc = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
    ];
    for pair in asc.windows(2) {
        let a = Version::parse(pair[0]).unwrap();
        let b = Version::parse(pair[1]).unwrap();
        assert!(a < b, "应满足 {a} < {b}（node-semver README 权威序列）");
    }
}

#[test]
fn max_satisfying_picks_highest_stable() {
    let candidates: Vec<Version> = ["1.2.3", "1.2.4", "1.3.0-beta.1", "1.2.2"]
        .iter()
        .map(|s| Version::parse(s).unwrap())
        .collect();
    let range = Range::parse("^1.2.0").unwrap();
    let got = range.max_satisfying(&candidates).map(|v| v.to_string());
    assert_eq!(got.as_deref(), Some("1.2.4"), "预发布应被守卫规则排除");
}
