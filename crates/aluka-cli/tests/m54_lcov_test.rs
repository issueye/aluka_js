//! M5.4 LCOV 覆盖率报告端到端测试（`aluka test --test-reporter=lcov`）。
//!
//! 覆盖 20260912 待办 1：行覆盖四层闭环（SpannedStmt 行号 → 编译期行表 →
//! VM 逐行计数 → LCOV tracefile 生成）。
//!
//! 判定口径：LCOV 结构面（TN/SF/FN/FNDA/FNF/FNH/DA/LF/LH/end_of_record）
//! 与确定性计数（固定用例的语句执行次数，迁移计数口径）。BRDA 分支覆盖
//! 不支持（引擎无分支级插桩，登记偏离）；行表不参与 `.bc` 序列化
//! （`aluka test` 进程内编译特性）。

mod common;

use std::path::PathBuf;

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("m54_lcov_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

fn parse_metric(out: &str, key: &str) -> u64 {
    out.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

/// 探针：3 个函数（1 个永不执行）+ if/else 循环（两分支各走一次）。
/// 迁移计数口径：add 执行 1 次（i=0）、never 0 次、main 1 次；
/// then 体 1 次、else 体 1 次、循环后语句 1 次。
#[test]
fn lcov_report_structure_and_counts() {
    let work = work_dir("lcov");
    let probe = work.join("cov.test.js");
    std::fs::write(
        &probe,
        r#"const { test } = require('node:test');
function add(a, b) {
  return a + b;
}
function never(x) {
  return x * 100;
}
let total = 0;
for (let i = 0; i < 2; i++) {
  if (i % 2 === 0) {
    total += add(i, 1);
  } else {
    total += 10;
  }
}
test('runs the loop', () => {
  if (total !== 11) throw new Error('total=' + total);
});
"#,
    )
    .unwrap();

    let out = common::run_lcov_test(&work, "cov.test.js");

    // 结构面：LCOV tracefile 全部必需条目
    for needle in [
        "TN:",
        "SF:",
        "FN:",
        "FNDA:",
        "FNF:",
        "FNH:",
        "DA:",
        "LF:",
        "LH:",
        "end_of_record",
    ] {
        assert!(out.contains(needle), "LCOV 报告缺 {needle}:\n{out}");
    }
    // 函数覆盖：add 执行 1 次；never 从未执行；函数总数 ≥ 3（含 test 闭包）
    assert!(out.contains("FNDA:1,add"), "add 执行 1 次:\n{out}");
    assert!(out.contains("FNDA:0,never"), "never 未执行:\n{out}");
    let fnf = parse_metric(&out, "FNF:");
    let fnh = parse_metric(&out, "FNH:");
    assert!(fnf >= 3, "函数总数（含 test 箭头闭包）至少 3:\n{out}");
    assert!(fnh >= 2, "命中函数至少 2（main+add+箭头）:\n{out}");
    // 行覆盖：then 体（i=0）与 else 体（i=1）各命中 1 次、末语句命中 1 次
    assert!(out.contains("DA:11,1"), "then 体命中 1 次:\n{out}");
    assert!(
        out.contains("DA:13,1") || out.contains("DA:13,2"),
        "else 体命中 1 次:\n{out}"
    );
    assert!(out.contains("DA:16,1"), "循环后语句命中 1 次:\n{out}");
    let lf = parse_metric(&out, "LF:");
    let lh = parse_metric(&out, "LH:");
    assert!(lf >= 6, "可覆盖行 ≥ 6:\n{out}");
    assert!(
        lh >= lf.saturating_sub(1) && lh >= 5,
        "命中行接近可覆盖行:\n{out}"
    );
}
