//! Phase 4 stream 家族内置库端到端与接口对拍测试：
//! - `stream`（Readable、Writable、pipe 管道流转、事件监听）
//! - `stream/promises`（finished、pipeline Promise 化异步支持）
//! - `stream/consumers`（text、json、buffer 流数据聚集消费）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase4_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 Readable 与 Writable 管道流转（pipe 与 finish 事件）
#[test]
fn readable_writable_pipe_e2e_matches_go() {
    let work = work_dir("pipe");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { Readable, Writable } = require(\"node:stream\");\n",
            "let chunks = [];\n",
            "const r = new Readable();\n",
            "const w = new Writable({\n",
            "    write(chunk) {\n",
            "        chunks.push(chunk);\n",
            "    }\n",
            "});\n",
            "w.on(\"finish\", () => {\n",
            "    console.log(\"pipe finished:\", chunks.join(\"\"));\n",
            "});\n",
            "r.pipe(w);\n",
            "r.push(\"hello \");\n",
            "r.push(\"stream \");\n",
            "r.push(\"pipe\");\n",
            "r.push(null);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "pipe finished: hello stream pipe");
}

/// 验证 stream/promises.finished 异步等待流完成
#[test]
fn stream_promises_finished_e2e_matches_go() {
    let work = work_dir("finished");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { Readable } = require(\"node:stream\");\n",
            "const { finished } = require(\"node:stream/promises\");\n",
            "async function main() {\n",
            "    const r = new Readable();\n",
            "    const p = finished(r);\n",
            "    r.push(\"alpha \");\n",
            "    r.push(\"beta\");\n",
            "    r.push(null);\n",
            "    r.resume();\n",
            "    await p;\n",
            "    console.log(\"promise finished ok\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "promise finished ok");
}

/// 验证 stream/consumers.text 将流数据聚集转为字符串
#[test]
fn stream_consumers_text_e2e_matches_go() {
    let work = work_dir("consumers_text");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { Readable } = require(\"node:stream\");\n",
            "const { text } = require(\"node:stream/consumers\");\n",
            "async function main() {\n",
            "    const r = new Readable();\n",
            "    r.push(\"stream \");\n",
            "    r.push(\"consumers \");\n",
            "    r.push(\"text\");\n",
            "    r.push(null);\n",
            "    const content = await text(r);\n",
            "    console.log(\"content:\", content);\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "content: stream consumers text");
}

/// `for await...of` 读取 Readable（同步缓冲 + 异步到达）
#[test]
fn readable_for_await_e2e_matches_go() {
    let work = work_dir("for_await");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { Readable } = require(\"node:stream\");\n",
            "async function main() {\n",
            "  const src = new Readable({ read() {} });\n",
            "  src.push(\"a\");\n",
            "  src.push(\"b\");\n",
            "  src.push(null);\n",
            "  let out = \"\";\n",
            "  for await (const c of src) { out += c; }\n",
            "  const src2 = new Readable({ read() {} });\n",
            "  setTimeout(() => { src2.push(\"x\"); src2.push(null); }, 5);\n",
            "  let out2 = \"\";\n",
            "  for await (const c of src2) { out2 += c; }\n",
            "  console.log(\"result: \" + out + \"|\" + out2);\n",
            "}\n",
            "main().catch(e => console.log(\"FAIL: \" + e.message));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "result: ab|x");
}
