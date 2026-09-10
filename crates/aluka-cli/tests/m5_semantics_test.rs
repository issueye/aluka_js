//! M5.4/M5.1 语义差分测试（对照 Node.js 22 LTS）。
//!
//! 覆盖本轮关闭的两处登记缺口：
//! - **M5.4 `t.mock.timers` 假时钟**：`enable({apis, now})` / `tick` / `setTime`
//!   / `runAll` / `reset` 的触发顺序与「不触发」语义；
//! - **M5.1 结构化克隆 5 处语义偏离**：getter 求值、Invalid Date、Error 实例
//!   （`instanceof` + `message`）、`SharedArrayBuffer` 进 transfer list 抛
//!   `DataCloneError`、未出现在图内的 transfer buffer 仍被 detach。
//!
//! 判定口径：`assert_e2e_matches_node` 会把 aluka 全链路（alukac 编译 → aluvm
//! 执行）的 stdout 与 `node` 的 stdout **逐字节比对**；差异即失败（不是跳过）。

mod common;

fn run_probe(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("m5_sem_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_node(&work, "probe.js")
}

/// 假时钟：`tick` 按到期顺序触发；跨度不足时不触发；`setTime` 只设时间不触发。
#[test]
fn timer_mock_tick_and_settime_matches_node() {
    let out = run_probe(
        "timer_tick",
        r#"
const { mock } = require('node:test');
const log = [];
mock.timers.enable({ apis: ['setTimeout'], now: 1000 });
setTimeout(() => log.push('T100'), 100);
setTimeout(() => log.push('T200'), 200);
mock.timers.tick(100);
log.push('after-tick-100');
mock.timers.tick(100);
log.push('after-tick-200');
mock.timers.setTime(9999);
log.push('after-setTime');
mock.timers.runAll();
log.push('after-runAll');
mock.timers.reset();
log.push('after-reset');
console.log(log.join(' '));
"#,
    );
    assert_eq!(
        out.trim(),
        "T100 after-tick-100 T200 after-tick-200 after-setTime after-runAll after-reset"
    );
}

/// 假时钟：`runAll` 补跑被 `setTime` 跳过（未到期）的定时器；`reset` 后真实定时器仍可用。
#[test]
fn timer_mock_runall_and_reset_matches_node() {
    let out = run_probe(
        "timer_runall",
        r#"
const { mock } = require('node:test');
const log = [];
mock.timers.enable({ apis: ['setTimeout'], now: 0 });
setTimeout(() => log.push('pend'), 5000);
mock.timers.setTime(1);
log.push('skipped-by-setTime:' + log.join(','));
mock.timers.runAll();
log.push('after-runAll');
mock.timers.reset();
setTimeout(() => {
  log.push('real-timer-fired');
  console.log(log.join(' '));
}, 0);
"#,
    );
    // 期望值 = Node 22 实测输出（`run_probe` 内已做过逐字节对拍，此处为二次锚定）
    assert_eq!(
        out.trim(),
        "skipped-by-setTime: pend after-runAll real-timer-fired"
    );
}

/// 假时钟是**引擎级单例**：模块级 `mock.timers` 与 `t.mock.timers` 控制同一份状态。
#[test]
fn timer_mock_shape_matches_node() {
    let out = run_probe(
        "timer_shape",
        r#"
const test = require('node:test');
const { mock } = require('node:test');
console.log('mock-same=' + (test.mock === mock));
console.log(typeof mock.timers.enable, typeof mock.timers.tick, typeof mock.timers.setTime, typeof mock.timers.runAll, typeof mock.timers.reset);
mock.timers.enable({ apis: ['setImmediate'], now: 0 });
let hit = 0;
setImmediate(() => { hit = 1; });
mock.timers.tick(0);
console.log('immediate-hit=' + hit);
mock.timers.reset();
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "mock-same=true");
    assert_eq!(lines[1], "function function function function function");
    assert_eq!(lines[2], "immediate-hit=1");
}

/// 结构化克隆：getter 被求值、Invalid Date 保真、Error 实例与 message 保真。
#[test]
fn clone_getter_invalid_date_error_matches_node() {
    let out = run_probe(
        "clone_three",
        r#"
console.log(JSON.stringify(structuredClone({ get a() { return 42; } })));
console.log(Number.isNaN(structuredClone(new Date(NaN)).getTime()));
const e = structuredClone(new Error('boom'));
console.log(e instanceof Error, e.message);
const te = structuredClone(new TypeError('bad'));
console.log(te instanceof Error, te instanceof TypeError, te.message);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], r#"{"a":42}"#);
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "true boom");
    assert_eq!(lines[3], "true true bad");
}

/// 结构化克隆：`SharedArrayBuffer` 进 transfer list → `DataCloneError`；
/// 直接克隆 SAB 不抛（Node 实测口径）。
#[test]
fn clone_shared_array_buffer_matches_node() {
    let out = run_probe(
        "clone_sab",
        r#"
try {
  const s = new SharedArrayBuffer(8);
  structuredClone({}, { transfer: [s] });
  console.log('sab-transfer: no-throw');
} catch (e) {
  console.log('sab-transfer: ' + e.name);
}
try {
  structuredClone(new SharedArrayBuffer(8));
  console.log('sab-direct: no-throw');
} catch (e) {
  console.log('sab-direct: ' + e.name);
}
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "sab-transfer: DataCloneError");
    assert_eq!(lines[1], "sab-direct: no-throw");
}

/// 结构化克隆：**未出现在被克隆值里**的 transfer buffer 仍必须被 detach。
#[test]
fn clone_unreachable_transfer_detached_matches_node() {
    let out = run_probe(
        "clone_detach",
        r#"
const b = new ArrayBuffer(8);
structuredClone({}, { transfer: [b] });
console.log('byteLength=' + b.byteLength);
const kept = new ArrayBuffer(8);
console.log('no-transfer byteLength=' + structuredClone(kept).byteLength + ', src=' + kept.byteLength);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "byteLength=0");
    assert_eq!(lines[1], "no-transfer byteLength=8, src=8");
}
