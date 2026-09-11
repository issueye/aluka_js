//! M5.2 `cluster` 生命周期事件端到端对拍测试（Node.js 22 LTS 为唯一权威，
//! 实测基线 v22.22.2）。
//!
//! 覆盖 primary 侧 `cluster` 的生命周期事件与 `worker.state` 全链路：
//!
//! - **`'fork'` 异步化**：Node `cluster.fork()` 内先写 `cluster.workers[id]`
//!   再 `process.nextTick(emitForkNT, worker)`，故 `fork()` 返回后的同步阶段
//!   事件计数为 0，且 `fork` 事件处 `state === 'none'`、`isConnected() === true`
//!   （Node `Worker.prototype.isConnected` 返回 `this.process.connected`，
//!   **与 `state` 无关**）；
//! - **`'online'`**：worker 上报后置 `state = 'online'`；
//! - **`'listening'`**：`cluster.emit('listening', worker, info)` 携 2 实参，
//!   `info` 的自有键为 `addressType/address/port/fd`（`fd` 恒 `undefined` 但
//!   为自有键），`state === 'listening'`；
//! - **`'disconnect'`**：`cluster.emit('disconnect', worker)` 仅 1 实参，
//!   `state === 'disconnected'`、`isConnected() === false`、`isDead() === false`、
//!   `exitedAfterDisconnect === false`（Node 在 disconnect 处做 `!!` 归一，
//!   把构造期的 `undefined` 收敛为 `false`），且 worker **仍在**
//!   `cluster.workers` 表中（Node 仅在 `isDead()` 时移除）；
//! - **`'exit'`**：`state === 'dead'`、worker 已从 `workers` 移除、`code` 为
//!   真实退出码（`process.exit(0)` → 0）、`signal === null`；
//! - **事件序**：`fork → online → listening → disconnect → exit`。
//!
//! **探针形态约束（两处，均为 Node 侧实测所得的确定性要求）**：
//! 1. worker 侧 `'listening'` 监听器须在 `listen()` **之后**注册，且关停+退出
//!    推迟到 `setImmediate`——同步 `srv.close()` 会清掉 cluster 的 index 键，
//!    使 Node `internal/cluster/child.js:119` 的 `indexes.has` 守卫短路掉
//!    上报帧（本运行时无该守卫，但两侧须同形才能成立对拍）；
//! 2. **不依赖 primary → worker 的消息投递**：本运行时 worker 侧
//!    `process.on('message')` 为空实现（已登记缺口），故探针让 worker 自行
//!    关停退出。
//!
//! **键序归一（已登记的既有偏离）**：本运行时 `Object.keys` 全仓按字典序
//! 输出，Node 按插入序。故 `info` 只断言键**集合**（`Object.keys(info).sort()`），
//! 不比较插入序——该偏离涉及解释器核心路径，属独立专项，不在 M5.2 内修。

mod common;

use std::path::{Path, PathBuf};

/// 创建隔离的临时测试目录。
fn work_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("m52_cluster_events_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 写探针源到工作目录（`.js`；bc 流水线由 harness 统一编译出 `.bc` 兄弟产物）。
fn write(work: &Path, name: &str, src: &str) {
    std::fs::write(work.join(name), src).expect("写探针失败");
}

// --- 1：完整生命周期事件链 --------------------------------------------------

/// `fork`（异步/none/connected）→ `online` → `listening`（payload）→
/// `disconnect`（1 实参、仍在 workers 表）→ `exit`（dead、已出表、code=0）。
#[test]
fn cluster_lifecycle_events_match_node() {
    let work = work_dir("lifecycle");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('cluster');

if (cluster.isPrimary) {
  let forkCount = 0;
  cluster.on('fork', function (w) {
    forkCount++;
    console.log(
      'EV fork id=' + w.id + ' state=' + w.state +
      ' connected=' + w.isConnected() + ' dead=' + w.isDead() +
      ' exitedAfterDisconnect=' + w.exitedAfterDisconnect +
      ' workers=' + Object.keys(cluster.workers).join(',') +
      ' in-table=' + (cluster.workers[w.id] === w)
    );
  });
  cluster.on('online', function (w) {
    console.log('EV online id=' + w.id + ' state=' + w.state + ' connected=' + w.isConnected());
  });
  cluster.on('listening', function (w, info) {
    console.log(
      'EV listening id=' + w.id + ' state=' + w.state + ' argc=' + arguments.length +
      ' connected=' + w.isConnected() +
      ' workers=' + Object.keys(cluster.workers).join(',')
    );
    console.log('  info.addressType=' + info.addressType + ' (typeof=' + typeof info.addressType + ')');
    console.log('  info.address=' + info.address);
    console.log('  info.port-is-ephemeral=' + (info.port >= 1024 && info.port <= 65535));
    console.log('  info.fd=' + info.fd + ' (typeof=' + typeof info.fd + ')');
    console.log('  info-keys=' + Object.keys(info).sort().join(','));
  });
  cluster.on('disconnect', function (w) {
    console.log(
      'EV disconnect id=' + w.id + ' state=' + w.state + ' argc=' + arguments.length +
      ' connected=' + w.isConnected() + ' dead=' + w.isDead() +
      ' exitedAfterDisconnect=' + w.exitedAfterDisconnect +
      ' workers=' + Object.keys(cluster.workers).join(',')
    );
  });
  cluster.on('exit', function (w, code, signal) {
    console.log(
      'EV exit id=' + w.id + ' code=' + code + ' signal=' + signal +
      ' state=' + w.state + ' dead=' + w.isDead() +
      ' workers=' + Object.keys(cluster.workers).join(',')
    );
  });
  cluster.fork();
  console.log('after-fork-sync fork-events-seen=' + forkCount);
} else {
  const net = require('net');
  const srv = net.createServer(function (c) { c.end('hi'); });
  // 'listening' 监听器在 listen() 之后注册；关停+退出推迟到 setImmediate
  // （同步 close 会短路 cluster 内部的 listening 上报）。
  srv.listen(0, '127.0.0.1');
  srv.on('listening', function () {
    setImmediate(function () {
      srv.close();
      process.exit(0);
    });
  });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    // Node 侧权威期望串（防「两侧同错」的假一致）。
    assert!(
        out.contains("after-fork-sync fork-events-seen=0"),
        "fork 必须异步发射（同步阶段计数为 0）:\n{out}"
    );
    assert!(
        out.contains(
            "EV fork id=1 state=none connected=true dead=false exitedAfterDisconnect=undefined workers=1 in-table=true"
        ),
        "fork 事件处：state=none / isConnected=true / workers 已写入:\n{out}"
    );
    assert!(
        out.contains("EV online id=1 state=online connected=true"),
        "online 事件:\n{out}"
    );
    assert!(
        out.contains("EV listening id=1 state=listening argc=2 connected=true workers=1"),
        "listening 事件（2 实参、state=listening）:\n{out}"
    );
    assert!(
        out.contains("  info.addressType=4 (typeof=number)"),
        "listening payload addressType:\n{out}"
    );
    assert!(
        out.contains("  info.address=127.0.0.1"),
        "listening payload address:\n{out}"
    );
    assert!(
        out.contains("  info.fd=undefined (typeof=undefined)"),
        "listening payload fd 为 undefined:\n{out}"
    );
    assert!(
        out.contains("  info-keys=address,addressType,fd,port"),
        "listening payload 键集合（含 fd 这一 undefined 自有键）:\n{out}"
    );
    assert!(
        out.contains(
            "EV disconnect id=1 state=disconnected argc=1 connected=false dead=false exitedAfterDisconnect=false workers=1"
        ),
        "disconnect 事件（1 实参、仍在 workers 表、exitedAfterDisconnect=false）:\n{out}"
    );
    assert!(
        out.contains("EV exit id=1 code=0 signal=null state=dead dead=true workers="),
        "exit 事件（code=0、state=dead、已出表）:\n{out}"
    );

    // 事件序：fork → online → listening → disconnect → exit（逐行定位）。
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    let seq = [
        "EV fork id=1",
        "EV online id=1",
        "EV listening id=1",
        "EV disconnect id=1",
        "EV exit id=1",
    ];
    let mut last = 0usize;
    for needle in seq {
        let at = idx(needle);
        assert!(
            at >= last,
            "事件序错误：{needle} 出现在前序事件之前:\n{out}"
        );
        last = at;
    }
    // `after-fork-sync` 必须早于 fork 事件（异步化的直接证据）。
    assert!(
        idx("after-fork-sync") < idx("EV fork id=1"),
        "fork 事件必须晚于同步阶段输出:\n{out}"
    );
}

// --- 2/3：listening payload 的 address / addressType 形态 -------------------

/// 未指定 host 的 `listen(0)`：Node 实测 `address === null`、`addressType === 4`
/// （`Server.prototype.listen` 的 `listenInCluster(this, null, port, 4, …)` 路径）。
#[test]
fn cluster_listening_payload_wildcard_matches_node() {
    let work = work_dir("wildcard");
    write(&work, "probe.js", &payload_probe("srv.listen(0);"));
    let out = common::assert_e2e_matches_node(&work, "probe.js");
    assert!(
        out.contains("addressType=4 (typeof=number) address=null port-ephemeral=true fd-own=true fd=undefined"),
        "未指定 host 时应为 address=null / addressType=4:\n{out}"
    );
    assert!(out.contains("EV exit code=0 state=dead"), "{out}");
}

/// 显式 `'0.0.0.0'`：Node 实测 `address === '0.0.0.0'`、`addressType === 4`。
#[test]
fn cluster_listening_payload_zero4_matches_node() {
    let work = work_dir("zero4");
    write(
        &work,
        "probe.js",
        &payload_probe("srv.listen(0, '0.0.0.0');"),
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");
    assert!(
        out.contains("addressType=4 (typeof=number) address=\"0.0.0.0\" port-ephemeral=true fd-own=true fd=undefined"),
        "显式 0.0.0.0 时应为 address=0.0.0.0 / addressType=4:\n{out}"
    );
}

/// 显式 `'::1'`：Node 实测 `address === '::1'`、`addressType === 6`。
#[test]
fn cluster_listening_payload_ipv6_matches_node() {
    let work = work_dir("ipv6");
    write(&work, "probe.js", &payload_probe("srv.listen(0, '::1');"));
    let out = common::assert_e2e_matches_node(&work, "probe.js");
    assert!(
        out.contains("addressType=6 (typeof=number) address=\"::1\" port-ephemeral=true fd-own=true fd=undefined"),
        "IPv6 监听时应为 addressType=6 / address=::1:\n{out}"
    );
}

/// 生成「单 worker 监听 → 自身上报 → 自行退出」的 payload 探针。
///
/// `listen_expr` 为 worker 侧实际调用的 `listen(...)` 表达式。
fn payload_probe(listen_expr: &str) -> String {
    format!(
        r#"
const cluster = require('cluster');

if (cluster.isPrimary) {{
  cluster.on('listening', function (w, info) {{
    console.log(
      'EV listening state=' + w.state +
      ' addressType=' + info.addressType + ' (typeof=' + typeof info.addressType + ')' +
      ' address=' + JSON.stringify(info.address) +
      ' port-ephemeral=' + (info.port >= 1024 && info.port <= 65535) +
      ' fd-own=' + Object.prototype.hasOwnProperty.call(info, 'fd') +
      ' fd=' + String(info.fd)
    );
  }});
  cluster.on('exit', function (w, code) {{
    console.log('EV exit code=' + code + ' state=' + w.state);
  }});
  cluster.fork();
  console.log('after-fork-sync');
}} else {{
  const net = require('net');
  const srv = net.createServer(function (c) {{ c.end('hi'); }});
  {listen_expr}
  srv.on('listening', function () {{
    setImmediate(function () {{
      srv.close();
      process.exit(0);
    }});
  }});
}}
"#
    )
}
