//! M5.2 worker 侧 IPC 面端到端对拍测试（Node.js 22 LTS 为唯一权威，实测基线 v22.23.1）。
//!
//! 覆盖 `20260911/README.md` §10「待办 28」的四项验收：
//!
//! 1. **primary → worker 消息投递**：`worker.send(v)` → worker 内
//!    `process.on('message', (message, handle))`，2 实参、`handle === undefined`、
//!    载荷 JSON 往返逐字一致；`process.listenerCount('message')` 含 Node 在
//!    `require('cluster')` 时挂接的内部桥接（实测：注册用户监听器前为 **1**、之后为 2）；
//! 2. **worker 侧 IPC 通道保活**：fork 出的子进程脚本跑完**不**退出（Node 实测：
//!    通道默认保活，`process.channel.unref()` 才立即释放），故 aluka 在通道建立即
//!    激活 IPC 事件源；`process.disconnect()` / 通道关闭后解除保活；
//! 3. **`process.disconnect()`（worker 侧）**：返回 `undefined`、`process.connected`
//!    **同步**翻 `false`（`'disconnect'` 事件本身**异步**派发——实测返回后调用栈内
//!    后续语句仍会执行）、二次调用抛 `ERR_IPC_DISCONNECTED`
//!    （文本 `IPC channel is already disconnected`）；primary 侧按
//!    `worker 'disconnect'`（0 实参）→ `cluster 'disconnect'`（1 实参）→ `'exit'`
//!    （`state='dead'`、已出 `workers` 表、`code=0`）派发；
//! 4. **worker 侧 `cluster.worker` 事件面**：`process` → `cluster.worker` 的单向桥接
//!    （`cluster.worker.on('message')` 收到同一载荷、`listenerCount('message')` 初值 0）、
//!    `cluster.worker.send` 回送、`isConnected()`/`isDead()`；
//! 5. **`process` 事件器值语义**（`on`/`addListener`/`once`/`off`/`removeListener`/
//!    `removeAllListeners`/`emit`/`listenerCount`/`listeners`）：`on` 返回 `process` 自身、
//!    别名共用同一函数对象（`off === removeListener`）、`emit` 返回是否有监听器、
//!    `once` 触发即自删。
//!
//! **探针纪律（均来自本轮实测踩坑，见 20260911/README.md §10.2）**：
//! 1. 一切「primary 先发」必须以 worker 的 `ready` 上报为门——`'online'` 到达 primary
//!    的时刻可能早于 worker 脚本注册 `process.on('message')`，先发的消息会被丢弃；
//! 2. primary 侧全部打印收拢进 `'exit'` 处理器——跨进程 stdout 无交错时序保证；
//! 3. worker 退出前不得留有在途帧：末次交互由 primary 的显式消息驱动且**不回送**；
//! 4. **worker 侧不在被杀前打印**：本运行时的 `console.log` 走行模型缓冲
//!    （`vm.stdout_records` 由 CLI 在运行结束后统一落盘），被 `kill()` 终止的子进程
//!    其缓冲不会落盘（与 Node 继承 stdio 的逐写直落差异，已登记为偏离）。

mod common;

use std::path::{Path, PathBuf};

/// 创建隔离的临时测试目录。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("m52_worker_msg_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 写探针源到工作目录（`.js`；bc 流水线由 harness 统一编译出 `.bc` 兄弟产物）。
fn write(work: &Path, name: &str, src: &str) {
    std::fs::write(work.join(name), src).expect("写探针失败");
}

// --- 1：primary → worker 消息投递 -------------------------------------------

/// `worker.send(v)` → worker `process.on('message')`：载荷/argc/handle/connected/
/// listenerCount（含 Node 内部桥接）+ 通道保活（未注册监听器前脚本跑完不退出）。
#[test]
fn cluster_worker_message_delivery_matches_node() {
    let work = work_dir("delivery");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');

if (cluster.isPrimary) {
  const rec = [];
  const w = cluster.fork();
  w.on('online', function () {
    rec.push('online id=' + w.id + ' state=' + w.state);
  });
  w.on('message', function (m, h) {
    if (m && m.ready) {
      rec.push('send1=' + w.send({ n: 1, s: 'str', arr: [1, '2', null, true], o: { k: 'v' } }));
      return;
    }
    rec.push('msg=' + JSON.stringify(m) + ' argc=' + arguments.length + ' handle=' + h);
    if (m && m.n === 1) rec.push('send2=' + w.send('plain'));
    else if (m === 'plain') rec.push('send3=' + w.send(42));
    else if (m === 42) rec.push('send4=' + w.send({ fin: true }));
  });
  w.on('exit', function (code, signal) {
    rec.push('exit code=' + code + ' signal=' + signal + ' state=' + w.state);
    for (const line of rec) console.log('P ' + line);
  });
} else {
  console.log('W listeners0=' + process.listenerCount('message') + ' on=' + typeof process.on);
  process.on('message', function (m, h) {
    console.log(
      'W msg=' + JSON.stringify(m) + ' argc=' + arguments.length + ' handle=' + h +
      ' connected=' + process.connected + ' listeners=' + process.listenerCount('message')
    );
    if (m && m.fin) {
      process.exit(0);
    } else {
      process.send(m);
    }
  });
  console.log('W listeners1=' + process.listenerCount('message'));
  process.send({ ready: true });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    // Node 侧权威期望串（防「两侧同错」的假一致）。
    assert!(
        out.contains("W listeners0=1 on=function"),
        "require('cluster') 在 worker 的 process 上留下内部 `'message'` 桥接监听器（Node 实测为 1）:\n{out}"
    );
    assert!(
        out.contains("W listeners1=2"),
        "用户注册后监听器数为 2:\n{out}"
    );
    assert!(
        out.contains(
            "W msg={\"n\":1,\"s\":\"str\",\"arr\":[1,\"2\",null,true],\"o\":{\"k\":\"v\"}} argc=2 handle=undefined connected=true listeners=2"
        ),
        "消息载荷/实参数/handle/connected 逐字段:\n{out}"
    );
    assert!(out.contains("W msg=\"plain\""), "字符串载荷:\n{out}");
    assert!(out.contains("W msg=42"), "数字载荷:\n{out}");
    assert!(
        out.contains("P send1=true") && out.contains("P send4=true"),
        "primary → worker 发送在通道就绪后恒为 true:\n{out}"
    );
    assert!(
        out.contains("P exit code=0 signal=null state=dead"),
        "worker 侧 process.exit(0) → primary 观测 code=0 / state=dead:\n{out}"
    );
    // 事件序：ready → send1 → 回送 → send2 → … → exit（逐行定位）。
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    assert!(
        idx("W listeners1=2") < idx("P online id=1")
            && idx("P send1=true") < idx("P msg={\"n\":1")
            && idx("P msg={\"n\":1") < idx("P send2=true"),
        "消息往返次序（ready 门控 + 逐轮回送）:\n{out}"
    );
}

// --- 2：`process.disconnect()`（worker 侧）-----------------------------------

/// worker `process.disconnect()`：返回值/`connected` 同步翻转/二次调用错误码 +
/// primary 侧 `'disconnect'`（worker 0 实参、cluster 1 实参）→ `'exit'` 序列。
#[test]
fn cluster_worker_process_disconnect_matches_node() {
    let work = work_dir("disconnect");
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
      rec.push('send-go=' + w.send({ go: true }));
      return;
    }
    rec.push('msg=' + JSON.stringify(m));
  });
  w.on('disconnect', function () {
    rec.push(
      'worker-disconnect state=' + w.state + ' argc=' + arguments.length +
      ' connected=' + w.isConnected() + ' dead=' + w.isDead() +
      ' exitedAfterDisconnect=' + w.exitedAfterDisconnect
    );
  });
  cluster.on('disconnect', function (worker) {
    rec.push('cluster-disconnect id=' + worker.id + ' argc=' + arguments.length);
  });
  w.on('exit', function (code, signal) {
    rec.push(
      'exit code=' + code + ' signal=' + signal + ' state=' + w.state +
      ' dead=' + w.isDead() + ' workers=' + Object.keys(cluster.workers).length
    );
    for (const line of rec) console.log('P ' + line);
  });
} else {
  process.on('message', function () {
    console.log('W disconnect-type=' + typeof process.disconnect + ' connected=' + process.connected);
    console.log('W ret=' + String(process.disconnect()) + ' connected-now=' + process.connected);
    try {
      process.disconnect();
      console.log('W second=no-throw');
    } catch (e) {
      console.log('W second code=' + e.code + ' msg=' + e.message);
    }
  });
  process.send({ ready: true });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    // Node 权威期望：返回 undefined + connected 同步翻 false + 二次调用错误码。
    // 关键：`'disconnect'` 为**异步**派发，故其后两条日志仍会被执行（同步发射会吞掉）。
    assert!(
        out.contains("W disconnect-type=function connected=true"),
        "worker 内 `process.disconnect` 存在且初始 connected=true:\n{out}"
    );
    assert!(
        out.contains("W ret=undefined connected-now=false"),
        "`process.disconnect()` 返回 undefined 且 connected 同步翻 false:\n{out}"
    );
    assert!(
        out.contains("W second code=ERR_IPC_DISCONNECTED msg=IPC channel is already disconnected"),
        "二次调用抛 ERR_IPC_DISCONNECTED（且 `'disconnect'` 未在调用栈内同步发射）:\n{out}"
    );
    assert!(
        out.contains("P send-go=true"),
        "通道就绪后 primary → worker 发送为 true:\n{out}"
    );
    assert!(
        out.contains(
            "P worker-disconnect state=disconnected argc=0 connected=false dead=false exitedAfterDisconnect=false"
        ),
        "primary 侧 worker 'disconnect'（0 实参、未 dead、exitedAfterDisconnect 归一为 false）:\n{out}"
    );
    assert!(
        out.contains("P cluster-disconnect id=1 argc=1"),
        "cluster 'disconnect' 携 1 实参:\n{out}"
    );
    assert!(
        out.contains("P exit code=0 signal=null state=dead dead=true workers=0"),
        "worker 自然退出（code=0）且已出 workers 表:\n{out}"
    );
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    assert!(
        idx("P worker-disconnect state=disconnected") < idx("P cluster-disconnect id=1")
            && idx("P cluster-disconnect id=1") < idx("P exit code=0"),
        "primary 侧事件序 disconnect(worker) → disconnect(cluster) → exit:\n{out}"
    );
}

// --- 3：worker 侧 `cluster.worker` 事件面 ------------------------------------

/// `cluster.worker.on('message')` 桥接（Node Worker ctor 的 process → worker 单向桥接）、
/// `cluster.worker.send` 回送、`listenerCount('message')` 初值 0、`isConnected`/`isDead`。
#[test]
fn cluster_worker_self_events_match_node() {
    let work = work_dir("self_events");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');

if (cluster.isPrimary) {
  const rec = [];
  const w = cluster.fork();
  w.on('online', function () {
    rec.push('online state=' + w.state + ' connected=' + w.isConnected());
  });
  w.on('message', function (m, h) {
    if (m && m.ready) {
      rec.push('send-ping=' + w.send({ ping: 1 }));
      return;
    }
    rec.push('msg=' + JSON.stringify(m) + ' argc=' + arguments.length + ' handle=' + h);
    if (m && m.pong) rec.push('send-bye=' + w.send({ bye: true }));
  });
  w.on('exit', function (code) {
    rec.push('exit code=' + code + ' state=' + w.state);
    for (const line of rec) console.log('P ' + line);
  });
} else {
  console.log(
    'W worker-state=' + cluster.worker.state +
    ' listeners=' + cluster.worker.listenerCount('message')
  );
  cluster.worker.on('message', function (m, h) {
    console.log(
      'W worker.on msg=' + JSON.stringify(m) + ' argc=' + arguments.length + ' handle=' + h +
      ' connected=' + cluster.worker.isConnected() + ' dead=' + cluster.worker.isDead()
    );
    if (m && m.ping) {
      console.log('W send-ret=' + String(cluster.worker.send({ pong: 1 })));
    } else {
      process.exit(0);
    }
  });
  console.log('W listeners-after=' + cluster.worker.listenerCount('message'));
  process.send({ ready: true });
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    assert!(
        out.contains("W worker-state=online listeners=0"),
        "worker 侧 `cluster.worker` 存在、state='online'、自身监听器数初值 0（桥接为单向 process→worker）:\n{out}"
    );
    assert!(
        out.contains("W listeners-after=1"),
        "注册后 worker 自身监听器数为 1:\n{out}"
    );
    assert!(
        out.contains(
            "W worker.on msg={\"ping\":1} argc=2 handle=undefined connected=true dead=false"
        ),
        "`cluster.worker.on('message')` 载荷/实参数/handle/isConnected/isDead:\n{out}"
    );
    assert!(
        out.contains("W send-ret=true"),
        "`cluster.worker.send` 回送为 true:\n{out}"
    );
    assert!(
        out.contains("W worker.on msg={\"bye\":true}"),
        "worker 事件面可持续接收:\n{out}"
    );
    assert!(
        out.contains("P msg={\"pong\":1} argc=2 handle=undefined"),
        "primary 侧收到 worker 回送（cluster 'message' 3 实参序的 worker/msg 部分）:\n{out}"
    );
    assert!(
        out.contains("P exit code=0 state=dead"),
        "worker 退出 code=0、state=dead:\n{out}"
    );
}

// --- 4：`process` 事件器值语义 ----------------------------------------------

/// `process` 事件面：`on`/`addListener` 返回 `process` 自身、别名同函数对象、
/// `emit` 返回是否有监听器、`once` 自删、`listeners` 副本、`removeAllListeners`。
#[test]
fn process_event_api_values_match_node() {
    let work = work_dir("process_events");
    write(
        &work,
        "probe.js",
        r#"
const order = [];
function a() { order.push('a'); }
function b() { order.push('b'); }

console.log('t1 count=' + process.listenerCount('x') + ' listeners=' + JSON.stringify(process.listeners('x').length));
console.log('t2 on-returns-self=' + (process.on('x', a) === process) + ' count=' + process.listenerCount('x'));
console.log('t3 add-returns-self=' + (process.addListener('x', b) === process) + ' count=' + process.listenerCount('x'));
console.log('t4 emit=' + process.emit('x') + ' order=' + order.join(','));
console.log('t5 off-returns-self=' + (process.off('x', a) === process) + ' count=' + process.listenerCount('x'));
console.log('t6 emit=' + process.emit('x') + ' order=' + order.join(','));
console.log('t7 listeners-first-is-b=' + (process.listeners('x')[0] === b));
console.log('t8 once-ret-self=' + (process.once('y', a) === process) + ' count=' + process.listenerCount('y'));
console.log('t9 emit-y=' + process.emit('y') + ' then=' + process.emit('y') + ' count=' + process.listenerCount('y') + ' order=' + order.join(','));
console.log('t10 aliases=' + (process.off === process.removeListener) + ',' + (process.addListener === process.on));
console.log('t11 emit-none=' + process.emit('nope') + ' empty-arg=' + process.emit());
console.log('t12 removeAll-self=' + (process.removeAllListeners('x') === process) + ' count=' + process.listenerCount('x') + ' y=' + process.listenerCount('y'));
console.log('t13 removeAll-all-self=' + (process.removeAllListeners() === process) + ' y=' + process.listenerCount('y'));
console.log('t14 emit-after-clear=' + process.emit('x') + ' ' + process.emit('y'));
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    assert!(
        out.contains("t1 count=0 listeners=0"),
        "初始无监听器:\n{out}"
    );
    assert!(
        out.contains("t2 on-returns-self=true count=1")
            && out.contains("t3 add-returns-self=true count=2"),
        "`on`/`addListener` 返回 process 自身且计数正确:\n{out}"
    );
    assert!(
        out.contains("t4 emit=true order=a,b"),
        "`emit` 返回 true 且按注册序调用:\n{out}"
    );
    assert!(
        out.contains("t5 off-returns-self=true count=1")
            && out.contains("t6 emit=true order=a,b,b"),
        "`off` 精确移除单个监听器:\n{out}"
    );
    assert!(
        out.contains("t7 listeners-first-is-b=true"),
        "`listeners()` 返回监听器副本（同一函数对象）:\n{out}"
    );
    assert!(
        out.contains("t9 emit-y=true then=false count=0 order=a,b,b,a"),
        "`once` 触发一次后自删（第二次 emit 返回 false）:\n{out}"
    );
    assert!(
        out.contains("t10 aliases=true,true"),
        "`off === removeListener`、`addListener === on`（Node 别名共用同一函数对象）:\n{out}"
    );
    assert!(
        out.contains("t11 emit-none=false empty-arg=false"),
        "无监听器 / 无实参时 emit 返回 false:\n{out}"
    );
    assert!(
        out.contains("t12 removeAll-self=true count=0 y=0")
            && out.contains("t13 removeAll-all-self=true y=0"),
        "`removeAllListeners(event)` / 全清均返回 process 自身:\n{out}"
    );
    assert!(
        out.contains("t14 emit-after-clear=false false"),
        "清空后 emit 返回 false:\n{out}"
    );
}

// --- 5：worker 侧 IPC 通道保活 ----------------------------------------------

/// fork 出的 worker **脚本跑完不退出**（Node 实测：IPC 通道默认保活；本运行时在通道
/// 建立即激活事件源）。判别力：若 worker 未保活，则其会在 ~ms 级退出，primary 在
/// 300ms 处观测到的 `connected` 必为 false 且 `'exit'` 早于该标记。
#[test]
fn cluster_worker_channel_keeps_alive_matches_node() {
    let work = work_dir("keep_alive");
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
  setTimeout(function () {
    rec.push(
      '@300ms connected=' + w.isConnected() + ' state=' + w.state +
      ' workers=' + Object.keys(cluster.workers).length
    );
    w.kill();
  }, 300);
  w.on('exit', function () {
    rec.push('exit-observed');
    for (const line of rec) console.log('P ' + line);
    process.exit(0);
  });
} else {
  // worker 侧刻意不打印（本运行时 console.log 走行模型缓冲，被 kill 的子进程
  // 其缓冲不会落盘——见文件头探针纪律 4）。
}
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");

    assert!(
        out.contains("P online state=online"),
        "worker 上报 online:\n{out}"
    );
    assert!(
        out.contains("P @300ms connected=true state=online workers=1"),
        "worker 脚本跑完 300ms 后仍存活（IPC 通道保活）:\n{out}"
    );
    assert!(
        out.contains("P exit-observed"),
        "primary 观测到 worker 退出:\n{out}"
    );
    let idx = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("缺行 {needle}:\n{out}"))
    };
    assert!(
        idx("P online") < idx("P @300ms") && idx("P @300ms") < idx("P exit-observed"),
        "次序 online → 300ms 存活标记 → exit:\n{out}"
    );
}
