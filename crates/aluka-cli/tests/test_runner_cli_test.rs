//! `aluka test` 子命令端到端测试（M5.4 切片一）：值调用形态、todo/skip 语义、
//! 报告器形态、退码与目录发现。
//!
//! 夹具写在 `std::env::temp_dir()` 下的隔离目录（测试名 + pid 唯一化，可并行
//! 互不覆盖）；被测进程仅为 `CARGO_BIN_EXE_aluka` 子进程——不依赖端口、时钟、
//! 网络或真实仓库布局，输出可逐字断言。
//!
//! 覆盖的 Node 22 实测锚点：
//! - `typeof require("node:test") === "function"` 且 `it === test`、
//!   `describe === suite`——**值调用** `test(name, fn)` 必须可用（缺陷 1）；
//! - 全通过文件退码 0、含失败文件退码 1，逐用例行 + 汇总块（spec/tap）；
//! - `test.todo(name)`（无回调）不执行、报告 `ok` + `TODO` 备注，退码 0；
//!   有回调的 todo 执行但失败不计入 `fail`、不影响退码（缺陷 2）；
//! - `it.skip`/`describe.skip` 函数属性形态与 options 形态逐字等价；
//! - 目录目标递归发现用例文件并忽略 `node_modules`。
//!
//! 报告格式口径：**本仓 CLI 契约**（`ok    name (SKIP)` / `ok N - name SKIP` /
//! `ℹ tests N` / `# tests N`），不声称与 `node --test` 输出逐字一致。

#![cfg(feature = "runtime")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// 隔离夹具目录（同名先清空；按测试名 + pid 唯一化）。
fn fixture_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aluka_tr_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 写入夹具脚本（自动建父目录）。
fn write_script(path: &Path, src: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("创建脚本目录失败");
    }
    std::fs::write(path, src).expect("写入脚本失败");
}

/// 一次 `aluka test` 子进程运行结果（退出码 + 归一化输出）。
struct TestRun {
    /// 退出码（被信号终止时为 None）。
    code: Option<i32>,
    /// stdout（`\r\n` 归一为 `\n`，保留尾换行）。
    stdout: String,
    /// stderr 原文（`\r\n` 归一）。
    stderr: String,
}

/// 运行 `aluka test <args...>`（cwd = 夹具目录）。
fn run_aluka_test(cwd: &Path, args: &[&str]) -> TestRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aluka"));
    cmd.arg("test").current_dir(cwd);
    for arg in args {
        cmd.arg(arg);
    }
    let out = cmd.output().expect("启动 aluka 失败");
    let norm = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n");
    TestRun {
        code: out.status.code(),
        stdout: norm(&out.stdout),
        stderr: norm(&out.stderr),
    }
}

/// 路径 → 子进程参数（夹具路径均为本机合法 UTF-8）。
fn arg(p: &Path) -> String {
    p.to_str().expect("路径为 UTF-8").to_owned()
}

/// spec 汇总块（本仓 CLI 契约，逐字；汇总记录自带前导换行 + `println!` 逐记录
/// 换行 → 块前恰有一个空行）。
fn spec_summary(tests: u32, pass: u32, fail: u32, skipped: u32, todo: u32) -> String {
    format!(
        "\n\nℹ tests {tests}\nℹ pass  {pass}\nℹ fail  {fail}\nℹ cancelled  0\nℹ skipped  {skipped}\nℹ todo  {todo}"
    )
}

/// 缺陷 1 锚点：模块导出值本身可调用（`const test = require("node:test")`），
/// 且 `it === test`、`test === test`（导出名）、`describe === suite`。
#[test]
fn value_call_and_function_identity_e2e() {
    let dir = fixture_dir("value_call");
    let probe = dir.join("probe.test.js");
    write_script(
        &probe,
        concat!(
            "const test = require(\"node:test\");\n",
            "console.log(\"typeof:\", typeof test);\n",
            "console.log(\"it===test:\", test.it === test, \"test===test:\", test.test === test);\n",
            "console.log(\"describe===suite:\", test.describe === test.suite);\n",
            "test(\"adds\", function () {});\n",
            "test.it(\"via-it\", function () {});\n",
        ),
    );
    let run = run_aluka_test(&dir, &[&arg(&probe)]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    let expected = format!(
        "typeof: function\n\
         it===test: true test===test: true\n\
         describe===suite: true\n\
         ok    adds\n\
         ok    via-it{}",
        spec_summary(2, 2, 0, 0, 0)
    );
    assert_eq!(run.stdout.trim_end(), expected);
}

/// 全通过文件 → 退码 0；含失败文件 → 退码 1；两者都打印逐用例行与汇总。
#[test]
fn pass_and_fail_files_drive_exit_code_e2e() {
    let dir = fixture_dir("exit_code");
    let ok = dir.join("ok.test.js");
    let bad = dir.join("bad.test.js");
    write_script(
        &ok,
        concat!(
            "const test = require(\"node:test\");\n",
            "test(\"case-a\", function () {});\n",
            "test(\"case-b\", function () {});\n",
        ),
    );
    write_script(
        &bad,
        concat!(
            "const test = require(\"node:test\");\n",
            "test(\"case-ok\", function () {});\n",
            "test(\"case-bad\", function () { throw new Error(\"boom\"); });\n",
        ),
    );

    let run_ok = run_aluka_test(&dir, &[&arg(&ok)]);
    assert_eq!(run_ok.code, Some(0), "stderr: {}", run_ok.stderr);
    assert_eq!(
        run_ok.stdout.trim_end(),
        format!("ok    case-a\nok    case-b{}", spec_summary(2, 2, 0, 0, 0))
    );

    let run_bad = run_aluka_test(&dir, &[&arg(&bad)]);
    assert_eq!(run_bad.code, Some(1), "stderr: {}", run_bad.stderr);
    // 失败行 = `not ok <name>` + 缩进后的错误消息（CLI 契约）
    assert!(
        run_bad.stdout.contains("not ok case-bad\n       boom"),
        "实际输出: {:?}",
        run_bad.stdout
    );
    assert!(
        run_bad.stdout.contains("\nℹ fail  1"),
        "实际输出: {:?}",
        run_bad.stdout
    );
}

/// `--test-reporter=tap`：逐用例行 `ok N - name` + `# tests N` 汇总块。
#[test]
fn tap_reporter_shape_e2e() {
    let dir = fixture_dir("tap");
    let probe = dir.join("probe.test.js");
    write_script(
        &probe,
        concat!(
            "const test = require(\"node:test\");\n",
            "test(\"case-a\", function () {});\n",
            "test.skip(\"case-skip\", function () {});\n",
            "test(\"case-bad\", function () { throw new Error(\"boom\"); });\n",
        ),
    );
    let run = run_aluka_test(&dir, &["--test-reporter=tap", &arg(&probe)]);
    assert_eq!(run.code, Some(1), "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout.trim_end(),
        "ok 1 - case-a\n\
         ok 2 - case-skip SKIP\n\
         not ok 3 - case-bad\n  ---\n  message: boom\n  ...\n\
         \n# tests 3\n# pass  1\n# fail  1\n# cancelled  0\n# skipped  1\n# todo  0"
    );
}

/// 缺陷 2 锚点：无回调的 todo 不执行、报告 `ok` + `TODO` 且退码 0；
/// 有回调的 todo 执行但失败不计入 `fail`；skip 一律不执行回调。
#[test]
fn todo_without_callback_is_ok_and_does_not_fail_exit_e2e() {
    let dir = fixture_dir("todo");
    let probe = dir.join("probe.test.js");
    write_script(
        &probe,
        concat!(
            "const test = require(\"node:test\");\n",
            // 无回调：不得去调用 undefined（修复前 → `undefined is not a function`）
            "test.todo(\"later work\");\n",
            // 有回调：执行，失败行状态 not ok + TODO，但 fail 不计
            "test.todo(\"todo-boom\", function () { throw new Error(\"x\"); });\n",
            // skip：不执行回调（throw 不得发生）
            "test.skip(\"skipme\", function () { throw new Error(\"no\"); });\n",
        ),
    );
    let run = run_aluka_test(&dir, &[&arg(&probe)]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout.trim_end(),
        format!(
            "ok    later work (TODO)\n\
             not ok todo-boom (TODO)\n       x\n\
             ok    skipme (SKIP){}",
            spec_summary(3, 0, 0, 1, 2)
        ),
        "无回调 todo 必须不执行且不计 fail（对齐 Node：pass 0/fail 0/skipped 1/todo 2，退码 0）"
    );

    // TAP 形态同一语义：状态与备注一致（文本形态按本仓契约）。
    let tap = run_aluka_test(&dir, &["--test-reporter=tap", &arg(&probe)]);
    assert_eq!(tap.code, Some(0), "stderr: {}", tap.stderr);
    assert!(
        tap.stdout.contains("ok 1 - later work TODO"),
        "实际输出: {:?}",
        tap.stdout
    );
    assert!(
        tap.stdout.contains("not ok 2 - todo-boom TODO"),
        "实际输出: {:?}",
        tap.stdout
    );
    assert!(
        tap.stdout.contains("ok 3 - skipme SKIP"),
        "实际输出: {:?}",
        tap.stdout
    );
    assert!(
        tap.stdout.contains("\n# fail  0"),
        "实际输出: {:?}",
        tap.stdout
    );
}

/// `it.skip`/`describe.skip` 的函数属性形态与 options 形态逐字等价。
#[test]
fn skip_function_attribute_and_options_forms_are_equivalent_e2e() {
    let dir = fixture_dir("skip_forms");
    let attr = dir.join("attr.test.js");
    let opts = dir.join("opts.test.js");
    // 函数属性形态：`it.skip(n, f)` / `describe.skip(n, f)`
    write_script(
        &attr,
        concat!(
            "const test = require(\"node:test\");\n",
            "test.it.skip(\"case-skip\", function () { throw new Error(\"must not run\"); });\n",
            "test.describe.skip(\"suite-marker\", function () {\n",
            "  test.it(\"inner\", function () { throw new Error(\"must not run\"); });\n",
            "});\n",
            "test.it(\"passes\", function () {});\n",
        ),
    );
    // options 形态：`it(n, { skip: true }, f)` / `describe(n, { skip: true }, f)`
    write_script(
        &opts,
        concat!(
            "const test = require(\"node:test\");\n",
            "test.it(\"case-skip\", { skip: true }, function () { throw new Error(\"must not run\"); });\n",
            "test.describe(\"suite-marker\", { skip: true }, function () {\n",
            "  test.it(\"inner\", function () { throw new Error(\"must not run\"); });\n",
            "});\n",
            "test.it(\"passes\", function () {});\n",
        ),
    );
    let run_attr = run_aluka_test(&dir, &[&arg(&attr)]);
    let run_opts = run_aluka_test(&dir, &[&arg(&opts)]);
    assert_eq!(run_attr.code, Some(0), "stderr: {}", run_attr.stderr);
    assert_eq!(run_opts.code, Some(0), "stderr: {}", run_opts.stderr);
    // 等价性：两份输出逐字一致（且 skip 回调确实未执行——否则会出现 fail 行）
    assert_eq!(
        run_attr.stdout.trim_end(),
        run_opts.stdout.trim_end(),
        "函数属性形态与 options 形态输出不一致"
    );
    assert_eq!(
        run_attr.stdout.trim_end(),
        format!(
            "ok    case-skip (SKIP)\n\
             ok    suite-marker > inner (SKIP)\n\
             ok    passes{}",
            spec_summary(3, 1, 0, 2, 0)
        )
    );
}

/// 目录目标递归发现用例文件（`*.test.js`/`*-test.js` + `test|tests` 目录下全部
/// 脚本），并忽略 `node_modules`。
#[test]
fn directory_target_recurses_and_ignores_node_modules_e2e() {
    let dir = fixture_dir("recursive");
    write_script(
        &dir.join("top.test.js"),
        "const test = require(\"node:test\");\ntest(\"case-top\", function () {});\n",
    );
    write_script(
        &dir.join("sub").join("nested-test.js"),
        "const test = require(\"node:test\");\ntest(\"case-nested\", function () {});\n",
    );
    // `tests/` 目录下不套命名约定也全部执行
    write_script(
        &dir.join("tests").join("any-name.js"),
        "const test = require(\"node:test\");\ntest(\"case-tests-dir\", function () {});\n",
    );
    // node_modules 下必须被忽略：该用例若被执行会令退码变 1，且名字可见
    write_script(
        &dir.join("node_modules").join("dep").join("ignored.test.js"),
        "const test = require(\"node:test\");\ntest(\"case-ignored\", function () { throw new Error(\"must not run\"); });\n",
    );

    let run = run_aluka_test(&dir, &[&arg(&dir)]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert!(
        run.stdout.contains("ok    case-top"),
        "实际输出: {:?}",
        run.stdout
    );
    assert!(
        run.stdout.contains("ok    case-nested"),
        "实际输出: {:?}",
        run.stdout
    );
    assert!(
        run.stdout.contains("ok    case-tests-dir"),
        "实际输出: {:?}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("case-ignored"),
        "node_modules 下的脚本必须被忽略，实际输出: {:?}",
        run.stdout
    );
    // 每个用例文件独立执行（Node「每文件独立进程」语义）→ 三个汇总块各 1 例。
    assert_eq!(
        run.stdout.matches("ℹ tests 1").count(),
        3,
        "三个被发现的用例文件应各自产出一份 1 例汇总，实际输出: {:?}",
        run.stdout
    );
}
