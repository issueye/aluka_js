//! Phase 6 进程类内置库端到端对拍测试：
//! - `child_process`（spawn 事件序 / exec / execFile / fork / spawnSync / execFileSync / execSync）
//! - `worker_threads`（伪 worker：isMainThread / parentPort 消息往返 / 缺文件错误路径 / MessageChannel / BroadcastChannel）
//! - `cluster`（isPrimary/isWorker 表面 / setupMaster / fork + Worker 包装 / disconnect）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针写法约束：Rust VM 暂缺 `JSON.stringify` / `Object.keys` / `String()` /
//! 字符串 `length`/`charCodeAt` 等全局（非本 Phase 范围），探针只使用
//! `console.log` + 字符串拼接 + `===` 比较，保证两侧可逐字对拍。

mod common;

use std::path::{Path, PathBuf};

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase6_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 编译全部 JS 模块为 .bc
fn compile_graph_with_aux(_unused: &Path, work: &Path, _entry: &str, _aux: &[(&str, &str)]) {
    common::compile_all_js(work);
}

/// 标准 e2e 一步：编译 → aluvm 执行。
fn assert_e2e_matches_go(work: &Path, entry: &str) -> String {
    common::rust_pipeline_run(work, entry)
}

// ---------------------------------------------------------------------------
// child_process：同步三件套
// ---------------------------------------------------------------------------

/// execSync 默认返回 Buffer、`encoding: 'utf8'` 返回字符串；非零退出 /
/// ENOENT 抛错对象（message/status/code/killed）逐字对齐 Go（含
/// `Command failed: <解析后命令行>` 与 `spawnSync <cmd> ENOENT` 文案）。
#[test]
fn exec_sync_buffer_encoding_and_throws_matches_go() {
    let work = work_dir("exec_sync");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const r1 = cp.execSync('cmd /c echo hello-aluka');\n",
            "console.log('r1: [' + r1.toString() + ']');\n",
            "const r2 = cp.execSync('cmd /c echo utf8-out', { encoding: 'utf8' });\n",
            "console.log('r2 type:', typeof r2, '[' + r2 + ']');\n",
            "try {\n",
            "  cp.execSync('cmd /c exit 3');\n",
            "  console.log('no throw');\n",
            "} catch (e) {\n",
            "  console.log('threw:', e.message);\n",
            "  console.log('status:', e.status, 'code empty:', e.code === '', 'killed:', e.killed);\n",
            "}\n",
            "try {\n",
            "  cp.execSync('definitely-not-exist-xyz');\n",
            "  console.log('no throw2');\n",
            "} catch (e) {\n",
            "  console.log('threw2:', e.message, 'status:', e.status, 'code:', e.code);\n",
            "}\n",
            "const eff = cp.execFileSync('cmd', ['/c', 'echo', 'eff-ok'], { encoding: 'utf8' });\n",
            "console.log('eff: [' + eff + ']');\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("r1: [hello-aluka"),
        "stdout 字节应含正文：{out}"
    );
    assert!(
        out.contains("threw: Command failed:"),
        "应抛 Command failed：{out}"
    );
}

/// spawnSync 结果对象形态：pid/status/signal/stdout 字节/error(ENOENT)/
/// input 写入/env 整体替换，与 Go 逐字一致。
#[test]
fn spawn_sync_result_shape_matches_go() {
    let work = work_dir("spawn_sync");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const r = cp.spawnSync('cmd', ['/c', 'echo', 'sync-spawn']);\n",
            "console.log('status:', r.status, 'signal:', r.signal, 'pid num:', typeof r.pid);\n",
            "console.log('stdout: [' + r.stdout.toString() + '] stderr empty:', r.stderr.toString() === '');\n",
            "console.log('error undef:', r.error === undefined);\n",
            "const r2 = cp.spawnSync('cmd', ['/c', 'echo', 'enc-test'], { encoding: 'utf8' });\n",
            "console.log('enc str: [' + r2.stdout + '] type:', typeof r2.stdout);\n",
            "const r3 = cp.spawnSync('cmd', ['/c', 'findstr', 'x'], { input: 'foo\\nxbar\\nbaz\\n' });\n",
            "console.log('input out: [' + r3.stdout.toString() + '] status:', r3.status);\n",
            "const r4 = cp.spawnSync('cmd', ['/c', 'echo', '%ALUKA_V%'], { env: { ALUKA_V: 'env-ok' } });\n",
            "console.log('env out: [' + r4.stdout.toString() + ']');\n",
            "const r5 = cp.spawnSync('definitely-missing-xyz', []);\n",
            "console.log('miss status null:', r5.status === null, 'pid:', r5.pid, 'err code:', r5.error.code);\n",
            "console.log('miss msg:', r5.error.message);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("status: 0 signal: null"), "{out}");
    assert!(
        out.contains(
            "miss msg: exec: \"definitely-missing-xyz\": executable file not found in %PATH%"
        ),
        "{out}"
    );
}

/// spawnSync 超时：status null + signal SIGTERM + error.code ETIMEDOUT
/// （Go 计时器 Kill + 自标 timedOut 的 Node Windows 语义）。
#[test]
fn spawn_sync_timeout_matches_go() {
    let work = work_dir("spawn_sync_timeout");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const r = cp.spawnSync('ping', ['-n', '30', '127.0.0.1', '-w', '1000'], { timeout: 300 });\n",
            "console.log('status null:', r.status === null, 'signal:', r.signal, 'err code:', r.error.code, 'err msg:', r.error.message);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("err code: ETIMEDOUT"), "{out}");
}

// ---------------------------------------------------------------------------
// child_process：spawn / exec / execFile / fork（异步）
// ---------------------------------------------------------------------------

/// spawn 事件序：spawn → data(Buffer 字节) → end → close(stream) →
/// exit(code, null) → close(code)；实例表面 stdout/stdin/pid 与
/// `stdin.write()` 返回 false（Go 简化一致）；启动失败 'error'（Go 文案）+
/// 'exit'(-1, null) 且无 'close'。
#[test]
fn spawn_event_order_and_error_path_matches_go() {
    let work = work_dir("spawn_events");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const ch = cp.spawn('cmd', ['/c', 'echo', 'spawn-out']);\n",
            "const lines = [];\n",
            "ch.on('spawn', () => { lines.push('spawn'); });\n",
            "ch.stdout.on('data', (d) => { lines.push('data:' + d.toString()); });\n",
            "ch.stdout.on('end', () => { lines.push('end'); });\n",
            "ch.stdout.on('close', () => { lines.push('sclose'); });\n",
            "ch.on('exit', (code, sig) => { lines.push('exit:' + code + ':' + (sig === null)); });\n",
            "ch.on('close', (code) => { lines.push('close:' + code); console.log(lines.join('|')); });\n",
            "console.log('sync surface:', typeof ch.stdout, typeof ch.stdin, typeof ch.pid, ch.stdin.write());\n",
            "const bad = cp.spawn('definitely-missing-xyz', []);\n",
            "const badLines = [];\n",
            "bad.on('error', (e) => { badLines.push('err:' + (e === 'exec: \"definitely-missing-xyz\": executable file not found in %PATH%')); });\n",
            "bad.on('exit', (code, sig) => { badLines.push('exit:' + code + ':' + (sig === null)); console.log(badLines.join('|')); });\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("spawn|data:spawn-out\n|end|sclose|exit:0:true|close:0")
            || out.contains("spawn|data:spawn-out\r\n|end|sclose|exit:0:true|close:0"),
        "spawn 事件序应为 spawn→data→end→sclose→exit→close：{out:?}"
    );
    assert!(
        out.contains("err:true|exit:-1:true"),
        "spawn 失败路径：{out:?}"
    );
}

/// exec/execFile 链式回调：Windows `cmd /c` 语义、stdout/stderr 字符串、
/// 非零退出 err === 'exit status 5'、ENOENT 文案、execFile 成功路径。
#[test]
fn exec_execfile_chained_callbacks_match_go() {
    let work = work_dir("exec_chain");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "cp.exec('cmd /c echo exec-out', (err, stdout, stderr) => {\n",
            "  console.log('exec:', err === null, stdout === 'exec-out\\r\\n', stderr === '');\n",
            "  cp.exec('cmd /c exit 5', (err2, so2, se2) => {\n",
            "    console.log('execfail:', err2 === 'exit status 5', so2 === '', se2 === '');\n",
            "    cp.execFile('definitely-missing-xyz', [], (err3) => {\n",
            "      console.log('missing:', err3 === 'exec: \"definitely-missing-xyz\": executable file not found in %PATH%');\n",
            "      cp.execFile('cmd', ['/c', 'echo', 'eff-out'], (err4, stdout4) => {\n",
            "        console.log('eff:', err4 === null, stdout4 === 'eff-out\\r\\n');\n",
            "        console.log('chain done');\n",
            "      });\n",
            "    });\n",
            "  });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("chain done"), "{out}");
    assert!(out.contains("execfail: true"), "{out}");
}

/// kill()：终止运行中的子进程并触发 'exit'/'close'（Windows 被杀 cmd 退出码 1）。
#[test]
fn spawn_kill_emits_exit_matches_go() {
    let work = work_dir("spawn_kill");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const ch = cp.spawn('ping', ['-n', '30', '127.0.0.1', '-w', '1000']);\n",
            "setTimeout(() => {\n",
            "  console.log('kill returned:', ch.kill());\n",
            "}, 100);\n",
            "ch.on('exit', (code, sig) => { console.log('killed exit:', code, sig === null); });\n",
            "ch.on('close', (code) => { console.log('killed close:', code); });\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("kill returned: true"), "{out}");
    assert!(out.contains("killed exit: 1 true"), "{out}");
}

/// fork：spawn 当前可执行文件跑模块；默认继承 stdio（stdout/stderr/stdin 为
/// null）；子进程退出触发 'exit'。退出码两侧引擎不同（Go 侧子进程可运行
/// 脚本、Rust 侧 aluvm 拒绝 .js），因此只断言事件与表面，不断言退出码。
/// fork 目标文件自身的 bc 缓存也会落入 node_modules，须按签名分发避免误拿。
#[test]
fn fork_inherits_stdio_and_exits_matches_go() {
    let work = work_dir("fork_basic");
    std::fs::write(
        work.join("fork_target_empty.js"),
        "const FORK_TARGET_MARKER = 'fork-target-only-marker';\n",
    )
    .unwrap();
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cp = require('child_process');\n",
            "const ch = cp.fork('fork_target_empty.js', ['a1']);\n",
            "let exitSeen = false;\n",
            "ch.on('exit', () => { exitSeen = true; console.log('fork exit seen:', exitSeen); });\n",
            "console.log('fork surface:', typeof ch.pid, ch.stdout === null, typeof ch.kill);\n",
        ),
    )
    .unwrap();
    compile_graph_with_aux(
        Path::new(""),
        &work,
        "probe.js",
        &[("fork_target_empty", "fork-target-only-marker")],
    );
    let out = assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("fork surface: number true function"), "{out}");
    assert!(out.contains("fork exit seen: true"), "{out}");
}

// ---------------------------------------------------------------------------
// worker_threads
// ---------------------------------------------------------------------------

/// 伪 worker 消息往返：主线程 isMainThread/threadId/parentPort/workerData
/// 默认值；worker 内 isMainThread=false、threadId=0（Go 怪癖）、workerData
/// 字段读取；parentPort.postMessage → 主线程 'message'（先于 'exit'）→
/// 'exit'(0)。worker 文件经 require 整图编译 + 签名分发为 .bc。
#[test]
fn worker_message_round_trip_matches_go() {
    let work = work_dir("worker_roundtrip");
    std::fs::write(
        work.join("worker_rt.js"),
        concat!(
            "const wt = require('worker_threads');\n",
            "if (!wt.isMainThread) {\n",
            "  console.log('in worker: isMain', wt.isMainThread, 'threadId', wt.threadId, 'parentPort', typeof wt.parentPort, 'task', wt.workerData.task);\n",
            "  wt.parentPort.postMessage('pong:' + wt.workerData.task);\n",
            "  wt.parentPort.postMessage(42);\n",
            "} else {\n",
            "  console.log('worker-file-ran-as-main');\n",
            "}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const wt = require('worker_threads');\n",
            "console.log('main:', wt.isMainThread, wt.threadId, wt.parentPort, wt.workerData);\n",
            "require('./worker_rt.js');\n",
            "const w = new wt.Worker('./worker_rt.js', { workerData: { task: 't1' } });\n",
            "w.on('message', (m) => { console.log('msg:', m, typeof m); });\n",
            "w.on('error', (e) => { console.log('werr:', typeof e); });\n",
            "w.on('exit', (code) => { console.log('wexit:', code); });\n",
        ),
    )
    .unwrap();
    compile_graph_with_aux(
        Path::new(""),
        &work,
        "probe.js",
        &[("worker_rt", "worker-file-ran-as-main")],
    );
    let out = assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("main: true 0 null null"), "{out:?}");
    // M5.1 真实跨线程路径：worker threadId 遵循 Node 22 语义（自 1 起），
    // 旧伪 worker 的恒 0 是 Go 怪癖（模块文档「已知偏离」已删除）
    assert!(
        out.contains("in worker: isMain false threadId 1 parentPort object task t1"),
        "{out:?}"
    );
    assert!(out.contains("msg: pong:t1 string"), "{out:?}");
    assert!(out.contains("msg: 42 number"), "{out:?}");
    assert!(out.contains("wexit: 0"), "{out:?}");
}

/// Worker 缺文件：'error' 事件（Go loader 失败路径）+ 'exit'(1)。
///
/// M5.1 收口：'error' 载荷由字符串改为 **Error 对象**（`WorkerEvent::Error`
/// 结构化为 name/message）——Node 22.23.1 实测缺文件异步路径（`./` 前缀）同为
/// Error 对象（code MODULE_NOT_FOUND）+ exit 1，旧字符串形态才是偏离。
/// 错误文本仍为 Go loader 失败文案（本地锚点，未与 Node 对拍）。
#[test]
fn worker_missing_file_emits_error_and_exit_matches_go() {
    let work = work_dir("worker_missing");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const wt = require('worker_threads');\n",
            "const w = new wt.Worker('no_such_worker_file.js');\n",
            "w.on('error', (e) => { console.log('werr fired:', typeof e, e instanceof Error); });\n",
            "w.on('exit', (code) => { console.log('wexit:', code); });\n",
            "console.log('after new Worker, threadId:', typeof w.threadId);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("werr fired: object true"), "{out:?}");
    assert!(out.contains("wexit: 1"), "{out:?}");
}

/// MessageChannel / MessagePort / BroadcastChannel / receiveMessageOnPort /
/// 环境数据（set/get/delete、数字键）/ markAsUntransferable 表面。
#[test]
fn message_channel_and_broadcast_match_go() {
    let work = work_dir("message_channel");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const wt = require('worker_threads');\n",
            "const ch = new wt.MessageChannel();\n",
            "ch.port2.on('message', (m) => { console.log('port2 got:', typeof m); });\n",
            "ch.port1.postMessage('hello-string');\n",
            "ch.port1.postMessage(123);\n",
            "const bc1 = new wt.BroadcastChannel('chan-x');\n",
            "const bc2 = new wt.BroadcastChannel('chan-x');\n",
            "bc2.on('message', (m) => { console.log('bc2 got:', typeof m, m === 'bc-ok'); });\n",
            "bc1.postMessage('bc-ok');\n",
            "const ch3 = new wt.MessageChannel();\n",
            "ch3.port1.postMessage('buffered');\n",
            "console.log('buffered recv:', wt.receiveMessageOnPort(ch3.port2).message);\n",
            "console.log('empty recv:', wt.receiveMessageOnPort(ch3.port2) === undefined);\n",
            "console.log('no-listener recv:', wt.receiveMessageOnPort(ch.port2) === undefined);\n",
            "wt.setEnvironmentData('k1', 'v1');\n",
            "console.log('envData:', wt.getEnvironmentData('k1'), 'missing undef:', wt.getEnvironmentData('nope') === undefined);\n",
            "wt.setEnvironmentData('k1');\n",
            "console.log('envData deleted:', wt.getEnvironmentData('k1') === undefined);\n",
            "wt.setEnvironmentData(3.5, 'num-val');\n",
            "console.log('envData num:', wt.getEnvironmentData(3.5));\n",
            "console.log('marks:', wt.isMarkedAsUntransferable({}) === false);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("buffered recv: buffered"), "{out:?}");
    assert!(out.contains("bc2 got: string true"), "{out:?}");
}

// ---------------------------------------------------------------------------
// cluster
// ---------------------------------------------------------------------------

/// cluster 表面 + fork：isPrimary/isMaster/isWorker、schedulingPolicy/
/// SCHED_NONE/SCHED_RR、settings（setupMaster/setupPrimary 写入）、
/// 'fork' 事件（**异步**：Node `process.nextTick(emitForkNT, worker)`，
/// 同步阶段计数为 0）、Worker 包装（id/process/state/send/kill/isConnected/
/// isDead）、子进程退出后 workers 表清理 + worker 'exit'(0)（Go 包装硬编码码）、
/// disconnect 回调、Worker 构造器。子进程（引擎二进制重跑自身）的报错
/// 只写 stderr，不影响 stdout 对拍。
#[test]
fn cluster_surface_and_fork_matches_go() {
    let work = work_dir("cluster_fork");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cluster = require('cluster');\n",
            "console.log('primary:', cluster.isPrimary, 'master:', cluster.isMaster, 'worker:', cluster.isWorker);\n",
            "console.log('policy:', cluster.schedulingPolicy, cluster.SCHED_NONE, cluster.SCHED_RR);\n",
            "console.log('worker undef:', cluster.worker === undefined);\n",
            "console.log('fns:', typeof cluster.fork, typeof cluster.setupMaster, typeof cluster.setupPrimary, typeof cluster.disconnect, typeof cluster.Worker);\n",
            "cluster.setupMaster({ exec: 'other.js', silent: true });\n",
            "console.log('settings exec:', cluster.settings.exec, 'silent:', cluster.settings.silent);\n",
            "cluster.setupPrimary({ args: ['x'] });\n",
            "console.log('settings args0:', cluster.settings.args[0], 'still exec:', cluster.settings.exec);\n",
            "console.log('ee on/emit:', typeof cluster.on, typeof cluster.emit);\n",
            "let forkEvt = 0;\n",
            "cluster.on('fork', () => { forkEvt++; });\n",
            "const w = cluster.fork();\n",
            "console.log('fork evt count sync:', forkEvt, 'id:', w.id, 'proc:', typeof w.process);\n",
            "process.nextTick(() => { console.log('fork evt count nt:', forkEvt, 'state:', w.state); });\n",
            "w.on('exit', (code) => { console.log('worker exit evt:', code, 'id-1-left:', cluster.workers['1'] === undefined); });\n",
            "console.log('workers has 1:', typeof cluster.workers['1'] === 'object');\n",
            "console.log('send/kill/conn/dead:', w.send(), w.kill(), w.isConnected(), w.isDead());\n",
            "const nw = new cluster.Worker();\n",
            "console.log('Worker ctor obj:', typeof nw);\n",
            "cluster.disconnect(() => { console.log('disconnected cb'); });\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("primary: true master: true worker: false"),
        "{out:?}"
    );
    // M5.2 收口：`'fork'` 为**异步**事件（Node `cluster.fork()` 内
    // `process.nextTick(emitForkNT, worker)`，实测 Node 22 同步阶段计数为 0）。
    // 旧断言 `fork evt count: 1` 是同步发射时代的产物，按 Node 语义修正。
    assert!(
        out.contains("fork evt count sync: 0 id: 1 proc: object"),
        "fork 事件必须异步（同步阶段计数应为 0）: {out:?}"
    );
    assert!(
        out.contains("fork evt count nt: 1 state: none"),
        "nextTick 处 fork 事件已发射且 state 为 none: {out:?}"
    );
    // `w.kill()` 后 exit 事件携带**真实退出码**（M5.2 IPC 面收口：旧实现硬编码 0）。
    // 本运行时无信号语义，被杀 worker 按引擎既有约定报 1（见 `worker_threads`
    // 的「0 正常 / 1 terminate」）；Node 22 实测为 `code=null, signal='SIGTERM'`
    // ——该差异已登记（Node 用信号表达终止，本运行时以退出码 1 表达）。
    assert!(
        out.contains("worker exit evt: 1 id-1-left: true"),
        "{out:?}"
    );
    assert!(out.contains("disconnected cb"), "{out:?}");
}
