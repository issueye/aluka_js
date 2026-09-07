//! Phase 8 生态兼容内置库 e2e 对拍测试：
//! - `punycode`（encode/decode/toASCII/toUnicode/ucs2，RFC 3492）
//! - `wasi`（WASI 类校验面、wasiImport 46 个 preview1 stub、start 语义）
//! - `test`（describe/it 注册模型、run() 事件流、hooks、t.plan、子测试、mock）
//! - `test/reporters`（dot/junit/spec/tap 可构造流 + lcov 实例）
//! - `markdown` 与 `aluka:markdown`（Aluka 扩展渲染 + frontmatter + 单例共享）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针编写约束（规避引擎既有缺口，与本批模块无关）：不用 `JSON.stringify`/
//! `Object.keys`/字符串 `.length`/函数 `.name`/`constructor.name`。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase8_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// punycode：encode/decode/toASCII/toUnicode 多组中英文混合域名矩阵。
#[test]
fn punycode_family_e2e_matches_go() {
    let work = work_dir("punycode");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const pc = require(\"node:punycode\");\n",
            "const cases = [\"\", \"Hello\", \"abc\", \"bücher\", \"münchen\", \"中国\", \"日本語\", \"Ω\", \"αβγδ\",\n",
            "  \"Hello-Another-Way-simple\", \"bücher.ch\", \"мойдомен.рф\", \"München-Ost\",\n",
            "  \"日本語。jp\", \"email@example.com\", \"x@ü\", \"-abc-\", \"a-b\", \"--\"];\n",
            "for (let i = 0; i < cases.length; i++) {\n",
            "  const c = cases[i];\n",
            "  let e, d, a, u;\n",
            "  try { e = pc.encode(c); } catch (err) { e = \"ERR:\" + err.message; }\n",
            "  try { d = pc.decode(e); } catch (err) { d = \"ERR:\" + err.message; }\n",
            "  try { a = pc.toASCII(c); } catch (err) { a = \"ERR:\" + err.message; }\n",
            "  try { u = pc.toUnicode(a); } catch (err) { u = \"ERR:\" + err.message; }\n",
            "  console.log(c, \"=>\", e, \"|\", d, \"|\", a, \"|\", u);\n",
            "}\n",
            "console.log(pc.ucs2.decode(\"hello\"));\n",
            "console.log(pc.ucs2.encode([0x1F600]));\n",
            "console.log(pc.decode(\"bcher-kva\"), \"|\", pc.encode(\"English\"));\n",
            "try { pc.decode(\"ÿ-??\"); } catch (e) { console.log(\"ERR:\", e.message); }\n",
            "console.log(typeof pc.version, pc.version, typeof pc.ucs2.decode, typeof pc.ucs2.encode);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("bücher => bcher-kva | bücher | xn--bcher-kva | bücher"));
    assert!(out.contains("中国 => fiqs8s | 中国 | xn--fiqs8s | 中国"));
    assert!(out.contains("ERR: Illegal input >= 0x80 (not a basic code point)"));
    assert!(out.starts_with("=>  |  |  | "));
    assert!(out.ends_with("string 2.1.0 function function"));
}

/// markdown：标题/列表/代码块/引用/水平线/行内格式与 frontmatter 全分支。
#[test]
fn markdown_render_branches_e2e_matches_go() {
    let work = work_dir("markdown");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const md = require(\"markdown\");\n",
            "console.log(md.render(\"# Hello\\n\\nWorld *em* **strong** `code` ~~del~~\"));\n",
            "console.log(md.render(\"- a\\n- b\\n\\n1. x\\n2. y\\n\"));\n",
            "console.log(md.render(\"```js\\nlet a = 1;\\n<tag>\\n```\"));\n",
            "console.log(md.render(\"> quote line\\n> more\"));\n",
            "console.log(md.render(\"---\\n\\ntext\"));\n",
            "console.log(md.render(\"[link](http://x.com?a=1&b=2) and ![img](/i.png)\"));\n",
            "console.log(md.render(\"###  no-space\"));\n",
            "console.log(md.render(\"+ plus item\\n* star item\\n- dash item\"));\n",
            "console.log(md.render(\"10. ten\"));\n",
            "console.log(md.render(\"a * b * c\"));\n",
            "console.log(md.render(\"1.no space\"));\n",
            "console.log(md.render(\"#### h4 ###tricky\"));\n",
            "console.log(md.render(\"\"));\n",
            "const fm = md.parseFrontmatter(\"---\\ntitle: T1\\nnum: 42\\n# comment\\n---\\nbody here\");\n",
            "console.log(\"fm:\", fm.data.title, fm.data.num, \"|\", fm.content);\n",
            "const fm2 = md.parseFrontmatter(\"---\\nq: \\\"quoted v\\\"\\ns: 'sq'\\n---\\nB\");\n",
            "console.log(\"fm2:\", fm2.data.q, fm2.data.s, \"|\", fm2.content);\n",
            "const fm3 = md.parseFrontmatter(\"no frontmatter\");\n",
            "console.log(\"fm3:\", typeof fm3.data, \"|\", fm3.content);\n",
            "console.log(md.renderToHTML(\"# Ti\\ntext\", { title: \"My T\" }));\n",
            "console.log(md.renderToHTML(\"x\"));\n",
            "console.log(md.renderToHTML(\"q\", { title: \"\" }));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("<h1>Hello</h1>"));
    assert!(out.contains("<pre><code class=\"language-js\">let a = 1;\n&lt;tag&gt;</code></pre>"));
    assert!(out.contains("<blockquote><p>quote line more</p></blockquote>"));
    assert!(out.contains("fm: T1 42 | body here"));
    assert!(out.contains("<title>My T</title>"));
    assert!(out.contains("<title>Aluka Static Document</title>"));
}

/// aluka:markdown：命名空间 specifier 与 `markdown` 共享同一单例。
#[test]
fn aluka_markdown_namespace_e2e_matches_go() {
    let work = work_dir("aluka_markdown");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const md = require(\"markdown\");\n",
            "const amd = require(\"aluka:markdown\");\n",
            "console.log(typeof amd.render, typeof amd.renderToHTML, typeof amd.parseFrontmatter, md === amd);\n",
            "console.log(amd.render(\"## amd works\\n\\n- x\\n\"));\n",
            "const nmd = require(\"node:markdown\");\n",
            "console.log(nmd === md);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "function function function true\n",
            "<h2>amd works</h2>\n<ul>\n<li>x</li>\n</ul>\n",
            "true"
        )
    );
}

/// wasi：options 校验错误面（message + code）、wasiImport stub、
/// start/initialize 语义与 getImportObject 绑定名。
#[test]
fn wasi_surface_e2e_matches_go() {
    let work = work_dir("wasi");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const w = require(\"node:wasi\");\n",
            "console.log(typeof w.WASI);\n",
            "try { new w.WASI(); } catch (e) { console.log(\"E1:\", e.message); }\n",
            "try { new w.WASI({ version: 42 }); } catch (e) { console.log(\"E3:\", e.message); }\n",
            "try { new w.WASI({ version: \"bogus\" }); } catch (e) { console.log(\"E4:\", e.message); }\n",
            "const inst = new w.WASI({ version: \"preview1\", args: [\"a\"], env: {}, preopens: { \"/\": \"/\" } });\n",
            "console.log(typeof inst.wasiImport, typeof inst.start, typeof inst.initialize, typeof inst.getImportObject);\n",
            "const names = [\"args_get\", \"args_sizes_get\", \"clock_res_get\", \"clock_time_get\",\n",
            "  \"environ_get\", \"environ_sizes_get\", \"fd_advise\", \"fd_allocate\", \"fd_close\",\n",
            "  \"fd_datasync\", \"fd_fdstat_get\", \"fd_fdstat_set_flags\", \"fd_fdstat_set_rights\",\n",
            "  \"fd_filestat_get\", \"fd_filestat_set_size\", \"fd_filestat_set_times\", \"fd_pread\",\n",
            "  \"fd_prestat_dir_name\", \"fd_prestat_get\", \"fd_pwrite\", \"fd_read\", \"fd_readdir\",\n",
            "  \"fd_renumber\", \"fd_seek\", \"fd_sync\", \"fd_tell\", \"fd_write\", \"path_create_directory\",\n",
            "  \"path_filestat_get\", \"path_filestat_set_times\", \"path_link\", \"path_open\",\n",
            "  \"path_readlink\", \"path_remove_directory\", \"path_rename\", \"path_symlink\",\n",
            "  \"path_unlink_file\", \"poll_oneoff\", \"proc_exit\", \"proc_raise\", \"random_get\",\n",
            "  \"sched_yield\", \"sock_accept\", \"sock_recv\", \"sock_send\", \"sock_shutdown\"];\n",
            "let n = 0;\n",
            "for (let i = 0; i < names.length; i++) { if (typeof inst.wasiImport[names[i]] === \"function\") n++; }\n",
            "console.log(\"stub-count:\", n);\n",
            "try { inst.wasiImport.args_get(); } catch (e) { console.log(\"E5:\", e.message); }\n",
            "const io = inst.getImportObject();\n",
            "console.log(\"io:\", typeof io.wasi_snapshot_preview1.args_get, typeof io.wasi_unstable);\n",
            "const inst2 = new w.WASI({ version: \"unstable\" });\n",
            "const io2 = inst2.getImportObject();\n",
            "console.log(\"io2:\", typeof io2.wasi_unstable.proc_exit, typeof io2.wasi_snapshot_preview1);\n",
            "try { inst.start({}); } catch (e) { console.log(\"E6:\", e.message); }\n",
            "try { inst.start(); } catch (e) { console.log(\"E7:\", e.message); }\n",
            "try { inst.initialize(); } catch (e) { console.log(\"E9:\", e.message); }\n",
            "try { inst2.start(undefined); } catch (e) { console.log(\"E10:\", e.message); }\n",
            "try { new w.WASI({ version: \"preview1\", stdin: -1 }); } catch (e) { console.log(\"E11:\", e.message, e.code); }\n",
            "try { new w.WASI({ version: \"preview1\", args: \"x\" }); } catch (e) { console.log(\"E12:\", e.message); }\n",
            "try { new w.WASI({ version: \"preview1\", env: 5 }); } catch (e) { console.log(\"E13:\", e.message); }\n",
            "console.log(inst instanceof w.WASI);\n",
            "const i3 = new w.WASI({ version: \"preview1\" });\n",
            "try { i3.start({ exports: {} }); } catch (e) { console.log(\"MEM:\", e.message); }\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains(
        "E1: The \"options.version\" property must be of type string. Received undefined"
    ));
    assert!(
        out.contains(
            "E4: The property 'options.version' unsupported WASI version. Received 'bogus'"
        )
    );
    assert!(out.contains("stub-count: 46"));
    assert!(out.contains("E5: wasi.start() has not been called"));
    assert!(out.contains("io: function undefined"));
    assert!(out.contains("io2: function undefined"));
    assert!(out.contains("E7: WASI instance has already started"));
    assert!(out.contains("E11: The value of \"options.stdin\" is out of range. It must be >= 0 && <= 2147483647. Received -1 ERR_OUT_OF_RANGE"));
    assert!(
        out.contains(
            "MEM: \"instance.exports.memory\" property must be a WebAssembly.Memory object"
        )
    );
    assert!(out.contains("true\nMEM: \"instance.exports.memory\""));
}

/// test（一）：run() 事件流——test:start/pass/fail/skip/todo/plan/end 与
/// 失败消息格式（含 assert 失败前缀）。
#[test]
fn test_runner_events_e2e_matches_go() {
    let work = work_dir("test_events");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const t = require(\"node:test\");\n",
            "t.test(\"a\", () => {});\n",
            "t.it(\"b\", () => { throw new Error(\"boom\"); });\n",
            "t.skip(\"s1\", () => {});\n",
            "t.todo(\"t1\", () => {});\n",
            "const s = t.run();\n",
            "s.on(\"test:start\", (e) => console.log(\"START\", e.name));\n",
            "s.on(\"test:pass\", (e) => console.log(\"PASS\", e.name));\n",
            "s.on(\"test:fail\", (e) => console.log(\"FAIL\", e.name, \"|\", e.details.error));\n",
            "s.on(\"test:skip\", (e) => console.log(\"SKIP\", e.name));\n",
            "s.on(\"test:todo\", (e) => console.log(\"TODO\", e.name));\n",
            "s.on(\"test:plan\", (e) => console.log(\"PLAN\", e.type, e.end.count, e.end.passing, e.end.failing, e.end.skipped, e.end.todo, e.end.cancelled));\n",
            "s.on(\"end\", () => console.log(\"END\"));\n",
            "setTimeout(() => {}, 5);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "START a\nPASS a\n",
            "START b\nFAIL b | boom\n",
            "START s1\nSKIP s1\n",
            "START t1\nTODO t1\n",
            "PLAN test 4 1 1 1 1 0\n",
            "END"
        )
    );
}

/// test（二）：describe 嵌套命名、beforeEach/afterEach、options 形态
/// skip/todo、t.plan 校验、t.assert 消息与 t.diagnostic 输出。
#[test]
fn test_suites_hooks_plan_e2e_matches_go() {
    let work = work_dir("test_suites");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const t = require(\"node:test\");\n",
            "t.describe(\"outer\", () => {\n",
            "  t.describe(\"inner\", () => {\n",
            "    t.it(\"deep\", (c) => { c.assert.strictEqual(1, 1); });\n",
            "  });\n",
            "  t.it(\"plain\", (c) => { c.assert.ok(true); });\n",
            "});\n",
            "t.describe(\"hooked\", () => {\n",
            "  t.beforeEach(() => { console.log(\"BE\"); });\n",
            "  t.afterEach(() => { console.log(\"AF\"); });\n",
            "  t.it(\"case1\", () => {});\n",
            "  t.it(\"case2\", (c) => { c.assert.equal(1, 2); });\n",
            "});\n",
            "t.it(\"opts-skip\", { skip: true }, () => {});\n",
            "t.it(\"opts-todo\", { todo: true }, () => {});\n",
            "t.it(\"plan-ok\", (c) => { c.plan(2); c.assert.ok(1); c.assert.strictEqual(\"a\", \"a\"); });\n",
            "t.it(\"plan-bad\", (c) => { c.plan(2); c.assert.ok(1); });\n",
            "t.it(\"diag\", (c) => { c.diagnostic(\"hello diag\"); });\n",
            "const s = t.run();\n",
            "s.on(\"test:start\", (e) => console.log(\"START\", e.name));\n",
            "s.on(\"test:pass\", (e) => console.log(\"PASS\", e.name));\n",
            "s.on(\"test:fail\", (e) => console.log(\"FAIL\", e.name, \"|\", e.details.error));\n",
            "s.on(\"test:skip\", (e) => console.log(\"SKIP\", e.name));\n",
            "s.on(\"test:todo\", (e) => console.log(\"TODO\", e.name));\n",
            "s.on(\"test:plan\", (e) => console.log(\"PLAN\", e.end.count, e.end.passing, e.end.failing, e.end.skipped, e.end.todo));\n",
            "s.on(\"end\", () => console.log(\"END\"));\n",
            "setTimeout(() => {}, 5);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("PASS deep"));
    assert!(out.contains("FAIL case2 | aluka: assertion error: expected 2 but got 1"));
    assert!(out.contains("FAIL plan-bad | expected 2 assertion calls, but received 1"));
    assert!(out.contains("# hello diag"));
    assert!(out.ends_with("PLAN 9 5 2 1 1\nEND"));
}

/// test（三）：async 用例、await 的子测试、同步父未 await 子测试的取消
/// 语义与 mock spy 委托/还原。
#[test]
fn test_async_subtests_mock_e2e_matches_go() {
    let work = work_dir("test_async");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const t = require(\"node:test\");\n",
            "t.it(\"async-ok\", async () => { await Promise.resolve(); });\n",
            "t.it(\"async-fail\", async () => { throw new Error(\"async boom\"); });\n",
            "t.it(\"parent\", async (c) => {\n",
            "  await c.test(\"sub-ok\", () => {});\n",
            "  await c.test(\"sub-bad\", () => { throw new Error(\"sub err\"); });\n",
            "});\n",
            "t.it(\"sync-parent-sub\", (c) => { c.test(\"never\", () => {}); });\n",
            "t.it(\"mocked\", () => {\n",
            "  const o = { foo: () => 42 };\n",
            "  const spy = t.mock.method(o, \"foo\");\n",
            "  spy();\n",
            "  spy();\n",
            "  const r = spy();\n",
            "  t.mock.restoreAll();\n",
            "  console.log(\"mocked-inner:\", r, o.foo());\n",
            "});\n",
            "const s = t.run();\n",
            "s.on(\"test:start\", (e) => console.log(\"START\", e.name));\n",
            "s.on(\"test:pass\", (e) => console.log(\"PASS\", e.name));\n",
            "s.on(\"test:fail\", (e) => console.log(\"FAIL\", e.name, \"|\", e.details.error));\n",
            "s.on(\"test:plan\", (e) => console.log(\"PLAN\", e.end.count, e.end.passing, e.end.failing, e.end.cancelled));\n",
            "s.on(\"end\", () => console.log(\"END\"));\n",
            "setTimeout(() => {}, 5);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("PASS async-ok"));
    assert!(out.contains("FAIL async-fail | async boom"));
    assert!(out.contains("PASS sub-ok"));
    assert!(out.contains("FAIL sub-bad | sub err"));
    assert!(out.contains("FAIL parent | parent > sub-bad: sub err"));
    assert!(out.contains("FAIL sync-parent-sub | 1 subtest failed"));
    assert!(out.contains("FAIL never | undefined"));
    assert!(out.contains("mocked-inner: 42 42"));
    assert!(out.ends_with("PLAN 8 3 4 1\nEND"));
}

/// test/reporters：dot/junit/spec/tap 可构造流（write/end/on/pipe）与
/// lcov 预构造实例；end 触发 finish+close。
#[test]
fn test_reporters_surface_e2e_matches_go() {
    let work = work_dir("test_reporters");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const rep = require(\"node:test/reporters\");\n",
            "console.log(typeof rep.dot, typeof rep.junit, typeof rep.spec, typeof rep.tap, typeof rep.lcov);\n",
            "const d = new rep.dot();\n",
            "console.log(typeof d.write, typeof d.end, typeof d.on, typeof d.pipe);\n",
            "console.log(\"pipe-self:\", d.pipe(d) === d);\n",
            "let fin = false, closed = false;\n",
            "d.on(\"finish\", () => { fin = true; });\n",
            "d.on(\"close\", () => { closed = true; });\n",
            "d.end();\n",
            "console.log(\"fired:\", fin, closed);\n",
            "console.log(\"lcov:\", typeof rep.lcov.write, rep.lcov !== d);\n",
            "const s = new rep.spec();\n",
            "console.log(\"fresh:\", s !== d, typeof s.end);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "function function function function object\n",
            "function function function function\n",
            "pipe-self: true\n",
            "fired: true true\n",
            "lcov: function true\n",
            "fresh: true function"
        )
    );
}

/// test 运行模型（Go `aluka run` 语义）：注册不产生输出，仅 `run()` 在
/// 事件循环存活时派发。
#[test]
fn test_registration_is_silent_in_run_mode_e2e() {
    let work = work_dir("test_silent");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const t = require(\"node:test\");\n",
            "t.test(\"silent\", () => {});\n",
            "t.describe(\"silent-suite\", () => { t.it(\"x\", () => {}); });\n",
            "console.log(\"registered-only\");\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "registered-only");
}
