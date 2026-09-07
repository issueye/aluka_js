//! Phase 4 内置库 `events` 端到端对拍测试：
//! - `require("events") === require("node:events")` 模块单例一致性
//! - `EventEmitter` 类构造器与 `defaultMaxListeners`
//! - `new EventEmitter()` 实例创建与 `on` / `emit` 触发
//! - `once` 单次监听与自动注销
//! - `off` / `removeListener` / `removeAllListeners` 监听器注销
//! - `listenerCount` 与 `setMaxListeners` / `getMaxListeners`
//!
//! 每一项均使用 common::assert_e2e_matches_go 与 Node.js 22 LTS 标准 逐字对拍。

mod common;

use std::path::PathBuf;

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "builtins_phase4_events_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录");
    dir
}

/// 测试 require("events") 与 require("node:events") 模块导出一致性及默认最大监听数
#[test]
fn events_module_identity_and_exports_e2e_matches_go() {
    let work = work_dir("identity");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const events1 = require(\"events\");\n",
            "const events2 = require(\"node:events\");\n",
            "console.log(events1 === events2);\n",
            "const { EventEmitter } = events1;\n",
            "console.log(typeof EventEmitter, typeof events1.EventEmitter);\n",
            "console.log(events1.defaultMaxListeners, EventEmitter.defaultMaxListeners);\n",
            "console.log(typeof events1.on, typeof events1.once, typeof events1.listenerCount);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "true\nfunction function\n10 10\nfunction function function"
    );
}

/// 测试 new EventEmitter()，监听 on 与 emit 触发多参回调
#[test]
fn events_emitter_on_and_emit_e2e_matches_go() {
    let work = work_dir("on_emit");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { EventEmitter } = require(\"node:events\");\n",
            "const ee = new EventEmitter();\n",
            "let sum = 0;\n",
            "ee.on(\"add\", (a, b) => {\n",
            "  sum += (a + b);\n",
            "});\n",
            "ee.addListener(\"add\", (a, b) => {\n",
            "  sum += (a * b);\n",
            "});\n",
            "const res1 = ee.emit(\"add\", 3, 4);\n",
            "console.log(sum, res1);\n",
            "const res2 = ee.emit(\"unknown\");\n",
            "console.log(res2);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "19 true\nfalse");
}

/// 测试 once 单次触发后自动注销
#[test]
fn events_emitter_once_e2e_matches_go() {
    let work = work_dir("once");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { EventEmitter } = require(\"node:events\");\n",
            "const ee = new EventEmitter();\n",
            "let count = 0;\n",
            "ee.once(\"ping\", () => {\n",
            "  count++;\n",
            "});\n",
            "console.log(ee.listenerCount(\"ping\"));\n",
            "ee.emit(\"ping\");\n",
            "console.log(count, ee.listenerCount(\"ping\"));\n",
            "ee.emit(\"ping\");\n",
            "console.log(count, ee.listenerCount(\"ping\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "1\n1 0\n1 0");
}

/// 测试 off 与 removeListener 移除指定监听器，以及 removeAllListeners 清空监听器
#[test]
fn events_emitter_off_and_remove_all_e2e_matches_go() {
    let work = work_dir("off_remove");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { EventEmitter } = require(\"node:events\");\n",
            "const ee = new EventEmitter();\n",
            "let triggered = false;\n",
            "const fn = () => { triggered = true; };\n",
            "ee.on(\"test\", fn);\n",
            "console.log(ee.listenerCount(\"test\"));\n",
            "ee.off(\"test\", fn);\n",
            "console.log(ee.listenerCount(\"test\"));\n",
            "ee.emit(\"test\");\n",
            "console.log(triggered);\n",
            "ee.on(\"e1\", () => {});\n",
            "ee.on(\"e2\", () => {});\n",
            "ee.removeAllListeners();\n",
            "console.log(ee.listenerCount(\"e1\"), ee.listenerCount(\"e2\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "1\n0\nfalse\n0 0");
}

/// 测试 listenerCount、defaultMaxListeners、setMaxListeners 与 getMaxListeners
#[test]
fn events_listener_count_and_max_listeners_e2e_matches_go() {
    let work = work_dir("counts");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const events = require(\"node:events\");\n",
            "const ee = new events.EventEmitter();\n",
            "console.log(ee.getMaxListeners());\n",
            "ee.setMaxListeners(25);\n",
            "console.log(ee.getMaxListeners());\n",
            "ee.on(\"data\", () => {});\n",
            "ee.on(\"data\", () => {});\n",
            "console.log(ee.listenerCount(\"data\"), events.listenerCount(ee, \"data\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "10\n25\n2 2");
}
