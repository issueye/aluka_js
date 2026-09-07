//! Phase 3 网络与重件基础设施内置库 e2e 对拍：
//! - `buffer`（Buffer 类、静态方法、实例属性/方法、切片、拼接）
//! - `perf_hooks`（performance.now、timeOrigin、mark、measure、getEntries、clearMarks）
//! - `v8`（getHeapStatistics 14 键、cachedDataVersionTag、serialize、deserialize）
//! - `timers` / `timers/promises`（定时器族与 Promise 化 setTimeout 异步 await）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。

mod common;

use std::path::PathBuf;

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase3_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录");
    dir
}

/// `buffer` 模块：属性、静态方法、实例方法、切片与拼接。
#[test]
fn buffer_family_e2e_matches_go() {
    let work = work_dir("buffer");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const bufMod = require(\"node:buffer\");\n",
            "const { Buffer } = bufMod;\n",
            "console.log(typeof Buffer, typeof bufMod.SlowBuffer, bufMod.kMaxLength, typeof bufMod.isUtf8, typeof bufMod.isAscii);\n",
            "const b1 = Buffer.from(\"hello\");\n",
            "console.log(Buffer.isBuffer(b1), b1.length, b1[0], b1[4], b1.toString());\n",
            "const b2 = Buffer.alloc(3, 65);\n",
            "console.log(b2.toString(), b2.length);\n",
            "const b3 = Buffer.concat([b1, b2]);\n",
            "console.log(b3.length, b3.toString());\n",
            "console.log(Buffer.byteLength(\"test\"), Buffer.isEncoding(\"utf8\"), Buffer.compare(b1, b1));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "function function 1073741824 function function\ntrue 5 104 111 hello\nAAA 3\n8 helloAAA\n4 true 0"
    );
}

/// `perf_hooks` 模块：now/timeOrigin/mark/measure/getEntries/clearMarks。
#[test]
fn perf_hooks_e2e_matches_go() {
    let work = work_dir("perf_hooks");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { performance } = require(\"node:perf_hooks\");\n",
            "console.log(typeof performance.now, typeof performance.timeOrigin);\n",
            "performance.mark(\"m1\");\n",
            "performance.measure(\"p1\", \"m1\");\n",
            "const entries = performance.getEntries();\n",
            "console.log(entries.length >= 2, typeof entries[0].name, typeof entries[0].startTime);\n",
            "performance.clearMarks(\"m1\");\n",
            "console.log(performance.getEntriesByType(\"mark\").length === 0);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "function number\ntrue string number\ntrue");
}

/// `v8` 模块：getHeapStatistics 14 规范统计键、cachedDataVersionTag 与序列化。
#[test]
fn v8_heap_statistics_e2e_matches_go() {
    let work = work_dir("v8");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const v8 = require(\"node:v8\");\n",
            "const s = v8.getHeapStatistics();\n",
            "console.log(typeof s.total_heap_size, typeof s.used_heap_size, typeof s.heap_size_limit, s.number_of_native_contexts);\n",
            "console.log(v8.cachedDataVersionTag(), typeof v8.serialize, typeof v8.deserialize);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "number number number 1\n0 function function");
}

/// `timers` 与 `timers/promises` 模块：定时器族与 Promise 化异步 await。
#[test]
fn timers_and_promises_e2e_matches_go() {
    let work = work_dir("timers");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const timers = require(\"node:timers\");\n",
            "console.log(typeof timers.setTimeout, typeof timers.clearTimeout, typeof timers.setInterval, typeof timers.clearInterval, typeof timers.setImmediate, typeof timers.clearImmediate);\n",
            "const tp = require(\"node:timers/promises\");\n",
            "console.log(typeof tp.setTimeout, typeof tp.setImmediate);\n",
            "async function main() {\n",
            "  const v = await tp.setTimeout(10, \"done\");\n",
            "  console.log(\"res:\", v);\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "function function function function function function\nfunction function\nres: done"
    );
}
