//! M5.4 真 `stream.Transform` 报告器差分测试（Node.js 22 LTS 实测口径 v22.23.1）。
//!
//! 覆盖 `20260911/README.md` §16「待办 32」：`run().compose(reporter).pipe(dest)`
//! 管道与报告器实例的 Transform 面。
//!
//! 判定口径：**结构化断言**（本仓报告契约文本，M5.4 切片一既定口径——不与
//! `node --test` reporter 逐字对拍：Node 侧 duration_ms/stack 本身非确定值）；
//! 表面常量（构造名/instanceof/导出名怪癖）取 Node 22.23.1 实测值。

mod common;

/// aluka 全链路执行（alukac 编译 → aluvm 执行）并返回 stdout。
fn run_aluka(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("m54_reporter_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_go(&work, "probe.js")
}

/// 报告器实例的 Transform 面（Node 22.23.1 实测常量）：
/// 导出四 function + lcov object；导出名怪癖（spec='value'、tap='tapReporter'、
/// junit='junitReporter'、dot='dot'）；spec 实例 `SpecReporter`、
/// `instanceof stream.Transform`、`writableObjectMode === true`；
/// `run().compose(spec)` 返回 `Readable`（有 `pipe`）。
#[test]
fn reporter_transform_surface_matches_node_oracle() {
    let out = run_aluka(
        "surface",
        r#"
const reporters = require('node:test/reporters');
const { Transform } = require('node:stream');
console.log('exports:', typeof reporters.dot, typeof reporters.junit, typeof reporters.spec, typeof reporters.tap, typeof reporters.lcov);
console.log('names:', reporters.dot.name, reporters.junit.name, reporters.spec.name, reporters.tap.name);
const spec = new reporters.spec();
console.log('spec-ctor:', spec.constructor.name, '| isTransform:', spec instanceof Transform, '| wObjMode:', spec.writableObjectMode);
console.log('methods:', typeof spec.write, typeof spec.end, typeof spec.on, typeof spec.pipe);
console.log('lcov:', typeof reporters.lcov, typeof reporters.lcov.write);
const { run } = require('node:test');
const composed = run().compose(reporters.spec);
console.log('composed:', composed.constructor.name, typeof composed.pipe);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        "exports: function function function function object"
    );
    // Node 内部导出名怪癖原样保留
    assert_eq!(lines[1], "names: dot junitReporter value tapReporter");
    assert_eq!(
        lines[2],
        "spec-ctor: SpecReporter | isTransform: true | wObjMode: true"
    );
    assert_eq!(lines[3], "methods: function function function function");
    assert_eq!(lines[4], "lcov: object function");
    assert_eq!(lines[5], "composed: Readable function");
}

/// `run().compose(reporters.tap).pipe(process.stdout)`：TAP 头、`# Subtest:`、
/// `ok N - name`/`not ok N - name`、失败 YAML 块、`1..N` 计划与 `# tests` 汇总。
#[test]
fn compose_tap_pipe_matches_tap_contract() {
    let out = run_aluka(
        "tap",
        r#"
const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('alpha', () => {});
test('beta', () => { throw new Error('boom'); });
test('gamma', () => {});
run().compose(reporters.tap).pipe(process.stdout);
"#,
    );
    assert!(out.contains("TAP version 13"), "TAP 头:\n{out}");
    assert!(out.contains("# Subtest: alpha"), "子测试声明行:\n{out}");
    assert!(out.contains("ok 1 - alpha"), "通过行:\n{out}");
    assert!(out.contains("not ok 2 - beta"), "失败行:\n{out}");
    assert!(out.contains("message: boom"), "失败 YAML 消息块:\n{out}");
    assert!(out.contains("ok 3 - gamma"), "序号连续:\n{out}");
    assert!(out.contains("1..3"), "计划行:\n{out}");
    assert!(out.contains("# tests 3"), "汇总 tests:\n{out}");
    assert!(out.contains("# pass  2"), "汇总 pass:\n{out}");
    assert!(out.contains("# fail  1"), "汇总 fail:\n{out}");
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    assert!(
        idx("ok 1 - alpha") < idx("not ok 2 - beta") && idx("not ok 2 - beta") < idx("1..3"),
        "TAP 行序：逐用例行先于计划行:\n{out}"
    );
}

/// `run().compose(reporters.spec).pipe(process.stdout)`：`ok`/`not ok` 行 +
/// `ℹ tests` 汇总块（本仓 spec 契约形态）。
#[test]
fn compose_spec_pipe_matches_spec_contract() {
    let out = run_aluka(
        "spec",
        r#"
const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('alpha', () => {});
test('beta', () => { throw new Error('boom'); });
run().compose(reporters.spec).pipe(process.stdout);
"#,
    );
    assert!(out.contains("ok    alpha"), "spec 通过行:\n{out}");
    assert!(out.contains("not ok beta"), "spec 失败行:\n{out}");
    assert!(out.contains("boom"), "失败错误文本:\n{out}");
    assert!(out.contains("ℹ tests 2"), "汇总 tests:\n{out}");
    assert!(out.contains("ℹ pass  1"), "汇总 pass:\n{out}");
    assert!(out.contains("ℹ fail  1"), "汇总 fail:\n{out}");
}

/// `run().compose(reporters.dot).pipe(process.stdout)`：逐用例 `.`/`X` 标记 +
/// 失败清单块。
#[test]
fn compose_dot_pipe_matches_dot_contract() {
    let out = run_aluka(
        "dot",
        r#"
const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('alpha', () => {});
test('beta', () => { throw new Error('boom'); });
test('gamma', () => {});
run().compose(reporters.dot).pipe(process.stdout);
"#,
    );
    let compact: String = out.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains(".X."),
        "dot 标记序（通过.失败X通过.）:\n{out}"
    );
    assert!(out.contains("Failed tests:"), "失败清单头:\n{out}");
    assert!(out.contains("beta"), "失败用例名:\n{out}");
}

/// pipe 晚于事件到达：compose 后立即结束事件流再 pipe——缓冲补冲不丢行。
#[test]
fn compose_pipe_after_events_flushes_buffer() {
    let out = run_aluka(
        "late_pipe",
        r#"
const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('only-one', () => {});
const composed = run().compose(reporters.tap);
composed.pipe(process.stdout);
setTimeout(() => {}, 0);
"#,
    );
    // 用例体在 run() 的宏任务中执行，pipe 已接好——直通输出
    assert!(out.contains("ok 1 - only-one"), "直通输出:\n{out}");
    assert!(out.contains("# tests 1"), "汇总:\n{out}");
}
