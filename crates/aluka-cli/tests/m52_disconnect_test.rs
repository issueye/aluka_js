//! M5.2 断连端到端对拍测试（Node.js 22 LTS 为唯一权威，实测基线 v22.23.1）。
//!
//! 覆盖 `20260911/README.md` §11「待办 29」项 1/2：
//!
//! 1. **primary 侧 `cluster.disconnect([cb])`**（Node `internal/cluster/primary.js`
//!    `cluster.disconnect` + `Worker.prototype.disconnect` + `removeWorker`）：
//!    `workers` 表**同步**清空（`workers-after=0`）、返回 `undefined`；每个 worker
//!    按 `worker 'disconnect'`（0 实参）→ `cluster 'disconnect'`（1 实参）→ `'exit'`
//!    派发，`ead=true`、`state` 依次为 `disconnected`/`dead`；`cb` 在全部 worker
//!    **出表后**触发（Node：`intercom.once('disconnect', cb)`）、0 实参、早于 `'exit'`；
//!    worker 内 `listen()` 的 server 被**关闭**（`'close'` 触发、`listening=false`），
//!    故 worker 能以 `code=0` 优雅退出；primary 之后自然退出（不靠 `process.exit`）。
//! 2. **worker 侧 `cluster.worker.disconnect()`**（Node `internal/cluster/child.js`
//!    `Worker.prototype.disconnect`）：**返回 `cluster.worker` 自身**、**同步**置
//!    `state='disconnecting'` 与 `exitedAfterDisconnect=true`、**同步**关闭本进程内
//!    server（`'close'` 在同一 tick、`setImmediate` 之前）；primary 侧观测
//!    `ead=true`、`'disconnect'` 时刻 worker **仍在** `workers` 表内
//!    （与 primary 发起时「立即出表」不同）。
//!
//! **探针纪律（本轮新增，均来自实测踩坑）**：
//! 1. **不使用块内函数声明**（`function f() {}` 写在 `if` 块内）：aluka 当前块内函数
//!    声明不可见（引擎级缺陷，已登记），Node 则可调用——探针一律用函数表达式；
//! 2. **固定单 worker**：多 worker 时 Node 侧「`cb` 与各 worker `'disconnect'` 事件」
//!    的相对顺序不稳定（实测 3 次有 1 次不同，跨进程竞态）；
//! 3. `cluster.on('disconnect')` 只注册一次（注册在 `for` 循环内会得到多份监听器，
//!    输出重复——首次取证即踩此坑）；
//! 4. worker 优雅退出，故其 `console.log` 可靠可见（与「被 `kill()` 终止的 worker
//!    缓冲不落盘」不同）；primary 侧仍收拢到 `'exit'` 之后打印。

mod common;

use std::path::{Path, PathBuf};

/// 创建隔离的临时测试目录。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("m52_disconnect_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 写探针源到工作目录（`.js`；bc 流水线由 harness 统一编译出 `.bc` 兄弟产物）。
fn write(work: &Path, name: &str, src: &str) {
    std::fs::write(work.join(name), src).expect("写探针失败");
}

// --- 1：primary 侧 `cluster.disconnect(cb)` ---------------------------------

/// primary 主动断连：workers 同步清空 + `cb` 时机 + server 关闭 + 优雅退出。
#[test]
fn cluster_primary_disconnect_matches_node() {
    let work = work_dir("primary");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');

if (cluster.isPrimary) {
  const rec = [];
  const start = function () {
    rec.push('before-disconnect workers=' + Object.keys(cluster.workers).length);
    const ret = cluster.disconnect(function () {
      rec.push('cb argc=' + arguments.length + ' workers=' + Object.keys(cluster.workers).length);
    });
    rec.push('disconnect-ret=' + String(ret) + ' workers-after=' + Object.keys(cluster.workers).length);
  };
  const w = cluster.fork();
  cluster.on('disconnect', function (worker) {
    rec.push('c-disconnect id=' + worker.id + ' argc=' + arguments.length);
  });
  w.on('online', function () {
    rec.push('online id=' + w.id + ' state=' + w.state);
  });
  w.on('message', function (m) {
    if (m && m.ready) {
      start();
      return;
    }
    rec.push('msg id=' + w.id + ' ' + JSON.stringify(m));
  });
  w.on('disconnect', function () {
    rec.push(
      'w-disconnect id=' + w.id + ' argc=' + arguments.length + ' state=' + w.state +
      ' ead=' + w.exitedAfterDisconnect + ' workers=' + Object.keys(cluster.workers).length
    );
  });
  w.on('exit', function (code, signal) {
    rec.push(
      'w-exit id=' + w.id + ' code=' + code + ' signal=' + signal +
      ' state=' + w.state + ' workers=' + Object.keys(cluster.workers).length
    );
    for (const line of rec) console.log('P ' + line);
  });
} else {
  const http = require('node:http');
  const srv = http.createServer(function (req, res) {
    res.end('hi');
  });
  srv.on('close', function () {
    console.log('W close id=' + cluster.worker.id);
  });
  srv.listen(0, '127.0.0.1', function () {
    console.log('W listening id=' + cluster.worker.id + ' listening=' + srv.listening);
    process.send({ ready: 1 });
  });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    // Node 侧权威期望串（防「两侧同错」的假一致）。
    assert!(
        out.contains("W listening id=1 listening=true"),
        "worker 内 http server 监听成功:\n{out}"
    );
    assert!(
        out.contains("W close id=1"),
        "断连时 worker 内 server 被关闭（`'close'` 派发）:\n{out}"
    );
    assert!(
        out.contains("P before-disconnect workers=1")
            && out.contains("P disconnect-ret=undefined workers-after=0"),
        "`cluster.disconnect()` 返回 undefined 且 workers 表**同步**清空:\n{out}"
    );
    assert!(
        out.contains("P w-disconnect id=1 argc=0 state=disconnected ead=true workers=0"),
        "worker 'disconnect'（0 实参、state=disconnected、ead=true、已出表）:\n{out}"
    );
    assert!(
        out.contains("P c-disconnect id=1 argc=1"),
        "cluster 'disconnect' 携 1 实参:\n{out}"
    );
    assert!(
        out.contains("P cb argc=0 workers=0"),
        "`cb` 以 0 实参触发、此刻 workers 已为 0:\n{out}"
    );
    assert!(
        out.contains("P w-exit id=1 code=0 signal=null state=dead workers=0"),
        "worker 优雅退出（code=0）且 state=dead:\n{out}"
    );
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    // 次序：worker 'disconnect' → cluster 'disconnect' → cb → 'exit'（Node 实测）。
    assert!(
        idx("P w-disconnect id=1") < idx("P c-disconnect id=1")
            && idx("P c-disconnect id=1") < idx("P cb argc=0")
            && idx("P cb argc=0") < idx("P w-exit id=1"),
        "断连事件序 disconnect(worker) → disconnect(cluster) → cb → exit:\n{out}"
    );
    assert!(
        idx("P disconnect-ret=") < idx("P w-disconnect id=1"),
        "`cluster.disconnect()` 返回早于异步的 'disconnect' 派发:\n{out}"
    );
}

// --- 2：worker 侧 `cluster.worker.disconnect()` ------------------------------

/// worker 主动断连：返回自身 + 同步置 `'disconnecting'`/`ead` + 同步关 server +
/// primary 侧 `'disconnect'` 时刻仍留在 `workers` 表。
#[test]
fn cluster_worker_self_disconnect_matches_node() {
    let work = work_dir("worker_self");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');

if (cluster.isPrimary) {
  const rec = [];
  const w = cluster.fork();
  w.on('online', function () {
    rec.push('online state=' + w.state);
  });
  w.on('message', function (m) {
    if (m && m.ready) {
      w.send({ go: 1 });
    } else {
      rec.push('msg=' + JSON.stringify(m));
    }
  });
  w.on('disconnect', function () {
    rec.push(
      'w-disconnect argc=' + arguments.length + ' state=' + w.state +
      ' ead=' + w.exitedAfterDisconnect + ' workers=' + Object.keys(cluster.workers).length
    );
  });
  cluster.on('disconnect', function (worker) {
    rec.push('c-disconnect id=' + worker.id + ' argc=' + arguments.length);
  });
  w.on('exit', function (code, signal) {
    rec.push(
      'w-exit code=' + code + ' signal=' + signal + ' state=' + w.state +
      ' workers=' + Object.keys(cluster.workers).length
    );
    for (const line of rec) console.log('P ' + line);
  });
} else {
  const http = require('node:http');
  const srv = http.createServer(function (req, res) {
    res.end('hi');
  });
  srv.on('close', function () {
    console.log('W close');
  });
  srv.listen(0, '127.0.0.1', function () {
    process.on('message', function (m) {
      if (!m.go) return;
      console.log(
        'W before state=' + cluster.worker.state + ' ead=' + cluster.worker.exitedAfterDisconnect +
        ' listening=' + srv.listening
      );
      const ret = cluster.worker.disconnect();
      console.log('W ret-is-self=' + (ret === cluster.worker) + ' ret-type=' + typeof ret);
      console.log(
        'W after state=' + cluster.worker.state + ' ead=' + cluster.worker.exitedAfterDisconnect +
        ' listening=' + srv.listening
      );
    });
    process.send({ ready: 1 });
  });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    assert!(
        out.contains("W before state=listening ead=undefined listening=true"),
        "调用前：state=listening、ead=undefined、server 监听中:\n{out}"
    );
    assert!(
        out.contains("W ret-is-self=true ret-type=object"),
        "`cluster.worker.disconnect()` **返回 worker 自身**（非 undefined/Promise）:\n{out}"
    );
    assert!(
        out.contains("W after state=disconnecting ead=true listening=false"),
        "**同步**置 state='disconnecting'、ead=true 并关闭本进程 server:\n{out}"
    );
    assert!(out.contains("W close"), "server 'close' 派发:\n{out}");
    assert!(
        out.contains("P w-disconnect argc=0 state=disconnected ead=true workers=1"),
        "worker 自己发起断连时 'disconnect' 时刻**仍在 workers 表内**（workers=1）:\n{out}"
    );
    assert!(
        out.contains("P c-disconnect id=1 argc=1"),
        "cluster 'disconnect' 携 1 实参:\n{out}"
    );
    assert!(
        out.contains("P w-exit code=0 signal=null state=dead workers=0"),
        "worker 优雅退出（code=0）、state=dead、已出表:\n{out}"
    );
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    assert!(
        idx("P w-disconnect argc=0") < idx("P c-disconnect id=1")
            && idx("P c-disconnect id=1") < idx("P w-exit code=0"),
        "事件序 disconnect(worker) → disconnect(cluster) → exit:\n{out}"
    );
}
