//! `cluster` 内置模块（Phase 6 / M5.2 IPC 面）。
//!
//! 语义逐字对齐 Node.js 22 LTS 规范（实测基线 v22.23.1）：
//! - 模块对象自带事件器表面（`on/once/emit/...`）：`'fork'`（**异步**，
//!   Node `process.nextTick(emitForkNT, worker)`）、`'online'`、`'listening'`、
//!   `'disconnect'`、`'exit'`、`'message'`；
//! - `isPrimary`/`isMaster`/`isWorker`：环境变量 `ALUKA_WORKER_ID` 标记 worker
//!   进程（worker 进程内另有 `worker = {id, send}`）；
//! - `workers`（id → Worker 实例）、`settings`、`schedulingPolicy`/`SCHED_NONE`(1)/`SCHED_RR`(2)；
//! - **`setupPrimary`/`setupMaster` settings 契约（M5.2）**：按 Node 源码语义
//!   `{ 默认值, ...旧 settings, ...options }` **重建** settings 对象（默认值 =
//!   `args: 主进程额外 CLI 参数`、`exec: 当前主脚本绝对路径`、`execArgv: []`、
//!   `silent: false`）；未知键同样保留、`undefined` 同样覆盖；
//! - **`fork()` 生效 `settings.exec` / `settings.args` / `silent` / `cwd`（M5.2）**：
//!   `fork()` 首行隐式 `setupPrimary()`（Node 行为），随后按 `createWorkerProcess`
//!   直取 settings 派生子进程——`exec` 为运行脚本、`args` 为实参尾串；`exec`
//!   非字符串或 `args` 非数组时，按 Node validator 文本抛
//!   `TypeError`(`code=ERR_INVALID_ARG_TYPE`)，且为 `fork()` 调用栈内**同步抛出**；
//! - `fork()`：复用 `child_process.fork` 派生当前可执行文件重跑当前脚本
//!   （Go 用 `os.Args[1]` 作脚本路径），并包一层 Worker 对象：child 的
//!   `'exit'` 事件转接到 Worker 与 cluster（携带**真实退出码**）；
//! - **IPC 面（M5.2）**：`cluster_ipc` 承载 Node 的 ipc 管道语义（见该模块
//!   文档）——worker 侧 `process.send` / `process.connected` / `process.disconnect`
//!   （Node bootstrap 阶段建立通道，与是否 require 本模块无关）、
//!   `process.on('message')`（**2 实参**：`(message, handle)`，无句柄传递故
//!   `handle` 恒 `undefined`）、`cluster.worker.send` 与其事件面
//!   （`'message'`/`'disconnect'` 由 process 侧单向桥接，Node `Worker` ctor 语义）；
//!   primary 侧 `worker.send`（写端真实可写性）、`'online'`（worker 通道建立后
//!   上报）、`'message'`（`(worker, message, handle)` 实参序）；
//! - **worker 侧 `require('cluster')` 的桥接（M5.2）**：进程内首次 require 时按
//!   Node `internal/cluster/child.js::_setupWorker` 挂接
//!   `process.on('message', (m,h) => worker.emit('message', m, h))` 与
//!   `process.once('disconnect', …)`（后者在 `exitedAfterDisconnect` 为假值时
//!   立即 `process.exit(0)`），故 `process.listenerCount('message')` 在用户注册前
//!   为 1（与 Node 实测一致）；
//! - **IPC 通道保活语义（M5.2 实测口径）**：Node 中 fork 出的子进程 IPC 通道
//!   **默认保活**（脚本跑完不退出，实测 14s 后仍 `connected=true`；
//!   `process.channel.unref()` 才立即释放），故 worker 侧在通道建立即激活
//!   `cluster_ipc` 事件源；`process.disconnect()` / 对端 EOF 后由泵注销；
//! - **`process.disconnect()`（worker 侧）**：返回 `undefined`、`process.connected`
//!   **同步**翻 `false`、二次调用抛 `ERR_IPC_DISCONNECTED`；`'disconnect'` 事件经
//!   nextTick **异步**派发（实测：返回后调用栈内后续语句仍会执行）；
//! - **primary 侧生命周期事件（M5.2）**：`worker.state` 全生命周期可观测
//!   （`'none'` → `'online'` → `'listening'` → `'disconnected'` → `'dead'`，
//!   Node `internal/cluster/primary.js` + `worker.js`）——
//!   `'fork'` 在 `fork()` 返回后经 nextTick 发射（此刻 `state='none'`、
//!   `isConnected()===true`、`workers[id]` 已写入）；
//!   worker 上报 listening 帧后置 `'listening'` 并派发
//!   `cluster.emit('listening', worker, info)`（`info` 自有键序
//!   `addressType,address,port,fd`，`fd` 恒 `undefined`）；
//!   IPC 通道关闭（或进程退出兜底）置 `'disconnected'` 并派发
//!   `worker.emit('disconnect')`（**无实参**）+ `cluster.emit('disconnect', worker)`
//!   （**1 实参**），此刻 worker 仍在 `workers` 表中；
//!   进程退出置 `'dead'` 并从 `workers` 移除，随后
//!   `cluster.emit('exit', worker, code, signal)`；
//! - **`disconnect([callback])`（M5.2）**：对每个 `isConnected()` 的 worker 走 primary
//!   侧 `Worker.prototype.disconnect()`（置 `exitedAfterDisconnect = true` → 发
//!   `{"t":"d"}` 帧 → **立即**把该 worker 移出 `workers` 表）；调用时表已空则走
//!   `process.nextTick(() => intercom.emit('disconnect'))`；`callback` 经
//!   `intercom.once('disconnect', cb)` **在 worker 循环之后**注册（Node 同序，故循环内
//!   `removeWorker` 的那次 emit 不被该 cb 消费），cb 在「`removeWorker` 令表变空」时
//!   以 **0 实参**同步触发（primary 发起断连的实际触发点在 worker 的 `'exit'` 转接内，
//!   故落在 `'disconnect'` 之后、`'exit'` 之前）；函数**无 return**（→ `undefined`）；
//!   `Worker` 构造器返回普通对象（供 instanceof 表面）。
//!
//! 如实登记的缺口（Node 22 行为未覆盖）：
//! - **`NODE_UNIQUE_ID`：不实现**——实测 Node 22 的 cluster worker 内
//!   `process.env.NODE_UNIQUE_ID` 为 `undefined`（旧评审把它列为缺失项是误判）；
//! - **worker 侧事件源保活粒度**：Node 的 channel ref 由「监听器计数」驱动
//!   （`refCounted`/`unrefCounted`）**且**子进程通道默认即保活；本运行时以
//!   「通道连通即保活」实现（与 Node 实测的默认行为等价），故
//!   `process.channel.ref()`/`unref()` 显式接口、`process.on('message')` 的
//!   计数式 ref 语义均未接线（含 `process.listeners` 之外的计数差异）；
//! - **worker 自发起断连的 `{"t":"e"}` ack 回程（20260911 实现）**：Node 的
//!   worker 侧 `_disconnect(false)` 先 `send({act:'exitedAfterDisconnect'})`，
//!   **等 primary 的 ack** 后才 `process.disconnect()`。本运行时 worker 上报
//!   `{"t":"e"}` 后**挂起**（`process.connected` 保持 `true`——Node 实测口径：
//!   `disconnect()` 同步返回后 `connected-sync === true`），primary 按
//!   `exitedAfterDisconnect(worker, message)` 置 `ead = true` 后以同帧
//!   `{"t":"e"}` 回程（Node 回 `{ack: message.seq}`，本运行时帧无 seq），worker
//!   收到 ack 才收尾断连；对端 EOF（primary 先死）时挂起失效，通道关闭路径
//!   自会派发 `'disconnect'`；
//! - **断连时关闭 worker 内 server**：`cluster.worker.disconnect()` 与收到的
//!   `{"t":"d"}` 帧都会关闭本进程内**全部监听中的** server（net 与 http 各自的
//!   线程局部表都要扫）并派发其 `'close'`（Node `_disconnect` 遍历 `handles`
//!   逐个 `close()`）；不清理则 worker 的事件循环不会排空、无法优雅退出；
//! - **worker 被杀时其 `console.log` 缓冲不落盘**：本运行时 `console.log` 走
//!   行模型（`vm.stdout_records`，由 CLI 在运行结束后统一输出），子进程被
//!   `kill()`/TerminateProcess 时缓冲丢失（Node 的继承 stdio 为逐写直落）。
//!   属引擎级既有行为，非本轮引入；探针须避免在「将被杀」的 worker 内打印；
//! - **primary 侧 `disconnect` 的触发源**：本运行时的「IPC 读线程」与「子进程
//!   退出事件源」是两条独立通路，退出转接处先**等本 worker 通道 EOF**（上限
//!   300ms 兜底）并排空在途帧，再按 Node 次序发 `'disconnect'` → `'exit'`；
//!   因此 worker 自行退出与 primary 发起断连的场景都与 Node 逐字段一致；
//! - **worker 侧 `cluster.worker.isDisconnected` 不实现**：Node 中仅经
//!   `deprecate()` 定义、并非自有属性；`'disconnecting'` 中间态由
//!   `cluster.worker.disconnect()` 同步写入（已接线）；
//! - **worker 侧 `cluster.worker.isDead()` 恒 `false`**：Node 取
//!   `process.exitCode != null || process.signalCode != null`，本运行时 worker 侧
//!   未接线二者（进程存活时二者恒为假值，故等价）；
//! - `listening` 帧的 `info.address`：Node 取 `dns.lookup` **解析后**的地址，
//!   本运行时原样回传请求的 host 串（显式 IP 完全一致；`'localhost'` 等
//!   别名不做解析，登记偏离）；未给 host 时两侧同为 `null`、`addressType` 4；
//! - 句柄（sendHandle）传递、`serialization: 'advanced'`、
//!   **真 round-robin 调度**（`schedulingPolicy` 恒 `SCHED_NONE`，端口由内核分发）；
//! - **`settings.execArgv` 不生效**：Node 把 execArgv 作为 node 旗标插在脚本之前
//!   （`node <execArgv...> <exec> <args...>`），本运行时进程形态为
//!   `aluka run <script>`，无对应旗标槽位；settings 中该键按 Node 默认写入
//!   `[]`，显式设置时不生效（登记偏离）；
//! - `settings.serialization` / `stdio` / `uid` / `gid` / `windowsHide` 未接线；
//! - **`settings.args` 为非数组对象时的 Node 特殊语义未复刻**：Node 会把该对象
//!   当作 `child_process.fork` 的 options（覆盖 `cwd`/`silent`/`stdio` 等），
//!   本运行时仅按「args 置空」处理（探针可覆盖的等价部分已对齐）；
//! - validator 的 `Received …` 描述实现 string/number/boolean/null/undefined/
//!   bigint/Symbol/Array/Object 形态；函数（Node 作 `Received function <name>`）
//!   与其它 exotic 形未复刻（登记偏离）。

use crate::builtins::child_process::proc_common::{
    ns_attach, ns_emit, ns_push_listener, register_ns_emitter_handlers,
};
use crate::builtins::child_process::{SpawnOpts, fork_spawn};
use crate::builtins::cluster_ipc;
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

/// `require("cluster")` / `require("node:cluster")` 模块导出。
pub const MODULE: ModuleDef = ModuleDef {
    name: "cluster",
    build,
};

// child 对象句柄 id → (worker id, worker 对象句柄 id)（事件转接用）；
// 线程局部：堆句柄仅本线程 Vm 有效。
thread_local! {
    static CHILD_TO_WORKER: RefCell<Option<HashMap<u32, (u64, u32)>>> =
        const { RefCell::new(None) };
}
fn with_child_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, (u64, u32)>) -> R,
{
    CHILD_TO_WORKER.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// Worker 生命周期状态（Node master 侧 `worker.state` 的完整取值面，
/// 见 `internal/cluster/worker.js` 的 `this.state = options.state || 'none'`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerPhase {
    /// `'none'`：通道已建立但尚未收到 worker 的 `'online'` 上报
    /// （实测 Node 22 `fork()` 返回时即为此态，而 `isConnected()` 已为 true）
    None,
    /// `'online'`：worker 上报后（`primary.js` 的 `online(worker)`）
    Online,
    /// `'listening'`：worker 上报 listening 帧后（`primary.js` 的 `listening()`）
    Listening,
    /// `'disconnected'`：IPC 通道已关闭但进程未退出
    Disconnected,
    /// `'dead'`：进程已退出
    Dead,
}

impl WorkerPhase {
    /// Node master 侧 `worker.state` 的字面值。
    fn as_state_str(self) -> &'static str {
        match self {
            WorkerPhase::None => "none",
            WorkerPhase::Online => "online",
            WorkerPhase::Listening => "listening",
            WorkerPhase::Disconnected => "disconnected",
            WorkerPhase::Dead => "dead",
        }
    }
}

// worker 对象句柄 id → 生命周期状态（`isConnected()`/`isDead()` 的真实来源）；
// 线程局部：堆句柄仅本线程 Vm 有效。
thread_local! {
    static WORKER_PHASE: RefCell<Option<HashMap<u32, WorkerPhase>>> =
        const { RefCell::new(None) };
}

fn with_phase_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, WorkerPhase>) -> R,
{
    WORKER_PHASE.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// 置某 worker 对象的状态：更新内部相表并**同步镜像** `worker.state`。
///
/// 偏离登记：Node master 侧 `state` 是普通数据属性（构造器直接赋值），本
/// 运行时以「写 Phase 即写属性」等价镜像——差异仅在 `worker.state = x` 的
/// 外部直写不会回写内部相表（Node 亦不会，`state` 非访问器，故行为一致）。
fn set_phase(vm: &mut Vm, worker_ref: u32, phase: WorkerPhase) {
    with_phase_map(|m| {
        m.insert(worker_ref, phase);
    });
    let state_str = vm.alloc_string(phase.as_state_str().to_owned());
    let _ = vm.set_property(
        Value::Object(ObjectRef(worker_ref)),
        "state",
        Value::Object(state_str),
    );
}

/// 读取某 worker 对象的状态（未知句柄视为未登记 → `None`）。
fn phase_of(worker_ref: u32) -> Option<WorkerPhase> {
    WORKER_PHASE.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&worker_ref).copied())
    })
}

/// `worker.isConnected()` 的真实判定——Node `Worker.prototype.isConnected`
/// 返回 `this.process.connected`（**通道连通性**，与 `state` 无关）：故
/// `'none'`（刚 fork、通道已建）亦为 `true`，仅通道关闭/进程退出后为 `false`。
fn phase_connected(phase: Option<WorkerPhase>) -> bool {
    matches!(
        phase,
        Some(WorkerPhase::None | WorkerPhase::Online | WorkerPhase::Listening)
    )
}

/// `worker.isDead()` 的真实判定（Node 由 `process.exitCode/signalCode` 表达，
/// 等价于 master 侧 `state === 'dead'`）。
fn phase_dead(phase: Option<WorkerPhase>) -> bool {
    matches!(phase, Some(WorkerPhase::Dead))
}

/// IPC 帧：worker 通道建立后上报 `'online'`（Node 子进程 setupChannel 阶段）。
const FRAME_ONLINE: &str = "{\"t\":\"o\"}";

/// primary → worker 的断连帧（Node `{act:'disconnect'}`）：worker 侧收到后走
/// `_disconnect(true)`（关本进程内 server → `process.disconnect()`）。
const FRAME_DISCONNECT: &str = "{\"t\":\"d\"}";

/// worker → primary 的「自发起断连」上报帧（Node `{act:'exitedAfterDisconnect'}`）：
/// primary 侧据此置 `exitedAfterDisconnect = true` 并**回同帧 ack**——worker 收到
/// 回程才收尾 `process.disconnect()`（Node `{ack: message.seq}` 的无 seq 近似，
/// 见 `cluster_ipc` 模块文档）。
const FRAME_EXITED_AFTER_DISCONNECT: &str = "{\"t\":\"e\"}";

/// worker → primary 的消息帧（`t: m`，载荷经 JSON 序列化，对齐 Node 默认
/// `serialization: 'json'`）。
fn frame_message(vm: &mut Vm, msg: Value) -> Result<String, VmError> {
    let payload = match vm.json_stringify(msg)? {
        // 顶层不可序列化值（undefined / 函数 / Symbol）：Node JSON 序列化下
        // 载荷缺失，此处以 null 承载，保证帧本身合法。
        Value::Undefined => "null".to_owned(),
        v => vm.format_value(v),
    };
    Ok(format!("{{\"t\":\"m\",\"v\":{payload}}}\n"))
}

/// 解析一帧为 `(类型, 载荷对象)`；帧非法（内部协议由握手 key 保护，正常不
/// 会发生）时返回 `None` 并丢弃，避免宿主侧事件泵抛出 JS 异常。
fn parse_frame(vm: &mut Vm, text: &str) -> Result<Option<(String, Value)>, VmError> {
    let text_val = Value::Object(vm.alloc_string(text.to_owned()));
    let Ok(frame) = vm.json_parse(&[text_val]) else {
        return Ok(None);
    };
    let Ok(t) = vm.get_property(frame, "t") else {
        return Ok(None);
    };
    Ok(Some((vm.format_value(t), frame)))
}

/// `require("cluster")` / `require("node:cluster")` 模块构建（模块单例）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // isPrimary / isMaster / isWorker（ALUKA_WORKER_ID 标记 worker 进程）。
    let worker_id_env = std::env::var("ALUKA_WORKER_ID").unwrap_or_default();
    let is_primary = worker_id_env.is_empty();
    let _ = vm.set_property(Value::Object(obj), "isPrimary", Value::Boolean(is_primary));
    let _ = vm.set_property(Value::Object(obj), "isMaster", Value::Boolean(is_primary));
    let _ = vm.set_property(Value::Object(obj), "isWorker", Value::Boolean(!is_primary));

    // worker 进程内：cluster.worker = {id, send, state, exitedAfterDisconnect}
    // ——`send` 与 `process.send` 同一条通道（Node `cluster.worker.send ===
    // fork 侧 process.send` 语义）；`state` 初值 `'online'`、`exitedAfterDisconnect`
    // 初值 `undefined`（Node `internal/cluster/child.js` 的 `_setupWorker`
    // 以 `state: 'online'` 构造 Worker）。
    if !is_primary {
        let worker_obj = vm.alloc_ordinary();
        let id: f64 = worker_id_env.parse().unwrap_or(1.0);
        let _ = vm.set_property(Value::Object(worker_obj), "id", Value::Number(id));
        let state_str = vm.alloc_string("online".to_owned());
        let _ = vm.set_property(Value::Object(worker_obj), "state", Value::Object(state_str));
        let _ = vm.set_property(
            Value::Object(worker_obj),
            "exitedAfterDisconnect",
            Value::Undefined,
        );
        // worker 自身事件面（Node `Worker` 继承 EventEmitter）：`send` 之外挂
        // emitter 方法；`'message'`/`'disconnect'` 由 process 侧桥接派发（见
        // `on_cluster_required` 与 `dispatch_self_frame`）。
        ns_attach(
            vm,
            worker_obj,
            "cluster:worker-self",
            &[
                "send",
                "disconnect",
                "on",
                "addListener",
                "once",
                "off",
                "removeListener",
                "removeAllListeners",
                "emit",
                "listenerCount",
            ],
        );
        register_ns_emitter_handlers(registry, "cluster:worker-self");
        set_worker_self(worker_obj.0);
        let _ = vm.set_property(Value::Object(obj), "worker", Value::Object(worker_obj));
    }
    // workers 表 / settings / schedulingPolicy 表面。
    let workers_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "workers", Value::Object(workers_obj));
    let settings_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "settings", Value::Object(settings_obj));
    let _ = vm.set_property(Value::Object(obj), "schedulingPolicy", Value::Number(1.0));
    let _ = vm.set_property(Value::Object(obj), "SCHED_NONE", Value::Number(1.0));
    let _ = vm.set_property(Value::Object(obj), "SCHED_RR", Value::Number(2.0));

    // 方法属性（分派经 module_of → "cluster.<method>"）。
    for method in [
        "fork",
        "setupMaster",
        "setupPrimary",
        "disconnect",
        "Worker",
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "removeAllListeners",
        "emit",
        "listenerCount",
    ] {
        let f = vm.alloc_native_fn(&format!("cluster.{method}"));
        set_module_prop(vm, obj, method, Value::Object(f))?;
    }
    register_ns_emitter_handlers(registry, "cluster");
    register_handler(registry, "cluster", "fork", cluster_fork);
    register_handler(registry, "cluster", "setupMaster", cluster_setup_master);
    register_handler(registry, "cluster", "setupPrimary", cluster_setup_master);
    register_handler(registry, "cluster", "disconnect", cluster_disconnect);
    register_handler(registry, "cluster", "Worker", cluster_worker_ctor);
    // 事件转接 wrapper（child 'exit' → worker + cluster 事件；消息面走 IPC 泵）。
    register_handler(registry, "cluster", "__workerExit", worker_exit_wrapper);
    // `'fork'` 事件的 nextTick 载体（Node `primary.js` 的 `emitForkNT`）：
    // 无接收者实参（`drain_microtasks` 以 `this === undefined` 调用），故待
    // 发射的 worker 句柄经线程局部队列传递。
    register_handler(registry, "cluster", "__emitForkNT", emit_fork_nt);
    // `process.send`（仅 worker 进程内挂属性，primary 侧为 undefined）：方法
    // 名经 NativeFn 全名 "process.send" 命中分派表，与是否 require cluster 无关，
    // 故在此模块（必定构建）登记。
    register_handler(registry, "process", "send", process_send);
    register_handler(registry, "process", "disconnect", process_disconnect);
    // `cluster.disconnect()` 在 `workers` 为空时的 nextTick 载体
    // （Node `process.nextTick(() => intercom.emit('disconnect'))`）。
    register_handler(
        registry,
        "cluster",
        "__intercomDisconnectNT",
        intercom_disconnect_nt,
    );
    // `process.disconnect()` 的 nextTick 载体（Node：`'disconnect'` 异步派发）。
    register_handler(
        registry,
        "cluster",
        "__selfDisconnectNT",
        self_disconnect_nt,
    );
    // worker 进程内 `cluster.worker.send`（同名空间只挂 send）与 Worker 方法面。
    register_handler(registry, "cluster:worker-self", "send", process_send);
    // worker 侧 `cluster.worker.disconnect()`（Node `child.js` 的
    // `Worker.prototype.disconnect`：置 `'disconnecting'` → `_disconnect()`）。
    register_handler(
        registry,
        "cluster:worker-self",
        "disconnect",
        worker_self_disconnect,
    );
    register_handler(
        registry,
        "cluster:worker-self",
        "isConnected",
        worker_self_is_connected,
    );
    register_handler(
        registry,
        "cluster:worker-self",
        "isDead",
        worker_self_is_dead,
    );
    // worker 进程内 `process.channel` 的开关面（`ref`/`unref`/`refCounted`/
    // `unrefCounted`：Node `lib/internal/child_process.js` 的 `Control` 类）。
    register_handler(registry, "cluster:channel", "ref", channel_ref);
    register_handler(registry, "cluster:channel", "unref", channel_unref);
    register_handler(
        registry,
        "cluster:channel",
        "refCounted",
        channel_ref_counted,
    );
    register_handler(
        registry,
        "cluster:channel",
        "unrefCounted",
        channel_unref_counted,
    );
    // worker 侧 process ↔ cluster.worker 桥接（Node `internal/cluster/worker.js`
    // 的 Worker ctor 与 `child.js` 的 `_setupWorker`）：经 require('cluster')
    // 挂接监听器（`on_cluster_required`），故处理器在此登记。
    register_handler(
        registry,
        "cluster",
        "__workerBridgeMessage",
        worker_bridge_message,
    );
    register_handler(
        registry,
        "cluster",
        "__workerBridgeDisconnect",
        worker_bridge_disconnect,
    );
    // Worker 实例方法命名空间。
    register_ns_emitter_handlers(registry, "cluster:worker");
    register_handler(registry, "cluster:worker", "send", worker_send);
    register_handler(registry, "cluster:worker", "kill", worker_kill);
    register_handler(registry, "cluster:worker", "destroy", worker_destroy);
    register_handler(
        registry,
        "cluster:worker",
        "isConnected",
        worker_is_connected,
    );
    register_handler(registry, "cluster:worker", "isDead", worker_is_dead);
    // primary 侧 `worker.disconnect()`（Node `primary.js` 的
    // `Worker.prototype.disconnect`：发 `d` 帧 + 出表 + `return this`）。
    register_handler(registry, "cluster:worker", "disconnect", worker_disconnect);
    Ok(obj)
}

/// `cluster.fork()`：child_process.fork 当前脚本 + Worker 包装。
fn cluster_fork(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    if !matches!(self_val, Value::Object(_)) {
        return Ok(self_val);
    }

    // Node：`cluster.fork()` 首行即 `cluster.setupPrimary()`（无参）——把默认值
    // 并入 settings 后再派生子进程（源码 `internal/cluster/primary.js:161-164`）。
    // 这一步同时保证「未调用过 setupPrimary 的裸 fork」也能从 settings 取到
    // exec/args（等价于旧行为：重跑当前脚本、不带参数）。
    cluster_setup_master(vm, &[])?;

    let settings = vm.get_property(self_val, "settings")?;
    // exec：Node `createWorkerProcess` 直取 `cluster.settings.exec`，缺失即
    // undefined → 由 `child_process.fork` 的 modulePath validator 抛 TypeError。
    let script = settings_exec(vm, settings)?;
    // args：Node 取 `cluster.settings.args`（数组），非数组同样由 validator 拒绝。
    let fork_args = settings_args(vm, settings)?;
    let settings_silent = read_silent(vm, settings);
    let settings_cwd = read_cwd(vm, settings);

    // worker id = 现有 workers 键数 + 1（Go len(workersObj.Keys())+1）。
    let worker_id = workers_count(vm, self_val) as u64 + 1;

    // IPC 监听器须在 spawn 之前绑定：端口 + 一次性握手 key 经环境变量注入子进程
    // （Node 的 ipc 管道在 stdio 数组里传递；本运行时以回环 TCP 承载同一语义）。
    let ipc = cluster_ipc::spawn_worker_listener(worker_id);

    // env：继承当前环境 + ALUKA_WORKER_ID 标记，用户传入 env 覆盖。
    let mut env_pairs: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.to_string_lossy().to_string(),
            )
        })
        .collect();
    env_pairs.push(("ALUKA_WORKER_ID".to_owned(), worker_id.to_string()));
    if let Some((port, key)) = &ipc {
        env_pairs.push((cluster_ipc::ENV_PORT.to_owned(), port.to_string()));
        env_pairs.push((cluster_ipc::ENV_KEY.to_owned(), key.clone()));
    }
    if let Some(user_env) = args.first().copied() {
        if let Value::Object(_) = user_env {
            for (k, v) in vm.own_properties(user_env) {
                env_pairs.push((k, vm.format_value(v)));
            }
        }
    }
    let opts = SpawnOpts {
        // Node：`silent: cluster.settings.silent`（缺省 false → 继承 stdio）。
        silent: Some(settings_silent),
        // Node：`cwd: cluster.settings.cwd`（缺省 undefined → 继承主进程 cwd；
        // 显式设置时相对 exec 亦按该目录解析——见 `read_cwd` 注释）。
        cwd: settings_cwd,
        windows_hide: cfg!(windows),
        env: Some(env_pairs),
    };

    let child_val = fork_spawn(vm, script, fork_args, opts)?;
    let Value::Object(child_ref) = child_val else {
        return Ok(child_val);
    };

    // Worker 对象（事件器语义）+ process 引用。
    let worker = vm.alloc_ordinary();
    ns_attach(
        vm,
        worker,
        "cluster:worker",
        &[
            "on",
            "once",
            "off",
            "emit",
            "listenerCount",
            "send",
            "kill",
            "destroy",
            "disconnect",
            "isConnected",
            "isDead",
        ],
    );
    let _ = vm.set_property(Value::Object(worker), "id", Value::Number(worker_id as f64));
    let _ = vm.set_property(Value::Object(worker), "process", child_val);
    with_child_map(|m| {
        m.insert(child_ref.0, (worker_id, worker.0));
    });
    // IPC 帧只带 worker id，故另存 id → 对象句柄的映射（消息/在线事件定位用）。
    register_worker_id(worker_id, worker.0);

    // 生命周期状态：Node `new Worker({id, process})` → `state = 'none'`
    // （`'online'` 由 worker 上报后置入）；`exitedAfterDisconnect` 初值 undefined
    // （`internal/cluster/worker.js:24`）。实测 oracle：`fork` 事件处
    // `state=none connected=true exitedAfterDisconnect=undefined`。
    set_phase(vm, worker.0, WorkerPhase::None);
    let _ = vm.set_property(
        Value::Object(worker),
        "exitedAfterDisconnect",
        Value::Undefined,
    );
    // child 'exit' → Worker/cluster 'exit'（携带真实退出码）；IPC 帧由事件源泵派发。
    attach_child_wrapper(vm, child_ref.0, "exit", "cluster.__workerExit")?;
    vm.activate_event_source("cluster_ipc", cluster_ipc_pump);

    // `workers[id] = worker` 必须**同步**写入：Node 在 `fork()` 返回前写表，
    // 随后才 `process.nextTick(emitForkNT, worker)`（实测 oracle：
    // `EV fork … workers=1 in-table=true`）。
    let workers_val = vm.get_property(self_val, "workers")?;
    let _ = vm.set_property(workers_val, &worker_id.to_string(), Value::Object(worker));
    // `'fork'` **异步**发射（Node `primary.js:196` `process.nextTick(emitForkNT, worker)`）：
    // 同步阶段事件计数为 0（实测 oracle `after-fork-sync fork-events-seen=0`）。
    let nt = vm.alloc_native_fn("cluster.__emitForkNT");
    with_pending_fork(|q| q.push_back(worker.0));
    vm.nexttick_queue.push_back(Value::Object(nt));
    Ok(Value::Object(worker))
}

/// child 事件上的内部转接监听器（原生函数注册进实例事件器监听器表）。
fn attach_child_wrapper(
    vm: &mut Vm,
    child_id: u32,
    event: &str,
    native_name: &str,
) -> Result<(), VmError> {
    let wrapper = vm.alloc_native_fn(native_name);
    ns_push_listener(child_id, event, Value::Object(wrapper));
    Ok(())
}

// 待异步发射 `'fork'` 的 worker 对象句柄队列（Node `process.nextTick(emitForkNT, worker)`
// 的等价物：nextTick 回调无接收者实参，故句柄经线程局部队列传递）。
thread_local! {
    static PENDING_FORK: RefCell<Option<VecDeque<u32>>> = const { RefCell::new(None) };
}

fn with_pending_fork<F, R>(f: F) -> R
where
    F: FnOnce(&mut VecDeque<u32>) -> R,
{
    PENDING_FORK.with(|g| f(g.borrow_mut().get_or_insert_with(VecDeque::new)))
}

/// `'fork'` 的 nextTick 载体：弹出队首 worker 并派发 `cluster.emit('fork', worker)`。
///
/// 队列为空（外部直接调用该内部函数）时静默返回。发射前校验 worker 仍在相表中，
/// 避免 worker 在 nextTick 之前就已退出时派发幽灵事件。
fn emit_fork_nt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(worker_ref) = with_pending_fork(|q| q.pop_front()) else {
        return Ok(Value::Undefined);
    };
    if phase_of(worker_ref).is_none() {
        return Ok(Value::Undefined);
    }
    if let Some(module_ref) = vm.builtin_registry.module("cluster") {
        ns_emit(
            vm,
            Value::Object(module_ref),
            "fork",
            &[Value::Object(ObjectRef(worker_ref))],
        )?;
    }
    Ok(Value::Undefined)
}

/// child 'exit' 转接：补发 `'disconnect'` → 清理 workers 表 → 置 Dead →
/// cluster `'exit'(worker, code, null)` → worker `'exit'(code, null)`。
/// 退出码取自 child `'exit'` 事件实参（真实子进程状态，不再是 Go 包装的硬编码 0）。
///
/// **`'disconnect'` 先于 `'exit'`**（Node：`worker.process.once('disconnect')` 排在
/// `once('exit')` 之前，实测 oracle 同为 disconnect → exit）。本运行时的 IPC 通道
/// EOF（读线程）与子进程退出事件是两条独立通路，故此处先等通道 EOF 并排空
/// inbox 在途帧，再按 Node 语义走 `'disconnect'` → `'exit'`
/// （`emit_disconnect` 对已 Disconnected/Dead 的 worker 幂等，通道先行 EOF 时
/// 不会重复派发）。
fn worker_exit_wrapper(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let child = current_receiver();
    let Value::Object(child_ref) = child else {
        return Ok(Value::Undefined);
    };
    let entry = with_child_map(|m| m.remove(&child_ref.0));
    let Some((worker_id, worker_ref)) = entry else {
        return Ok(Value::Undefined);
    };
    // cluster 模块对象：从分派表反查（模块单例）。
    let Some(module_ref) = vm.builtin_registry.module("cluster") else {
        return Ok(Value::Undefined);
    };
    let module_val = Value::Object(module_ref);
    // 次序钉死（对齐 Node：通道 EOF 产生的 `'disconnect'` 先于 `'exit'`）：
    // 本运行时「IPC 读线程」与「子进程退出事件源」是两条独立通路，故先等本
    // worker 的通道 EOF（子进程已退出，通常 <1ms；上限 300ms 兜底），再把
    // inbox 中在途帧（online/listening/message）派发干净，随后才发
    // `'disconnect'`/`'exit'`——既保证次序，也保证 listening 帧不被 close 抢先丢弃。
    //
    // 判据用 `listener_spawned`（而非「是否曾握手成功」）：accept 线程可能尚未
    // 处理完握手，此刻「尚未建立」并不代表「对端不会连接」，若据此跳过等待就会
    // 把已在内核缓冲区里的帧永久搁浅（该 worker 已 Dead，事件源随后注销）。
    if cluster_ipc::listener_spawned(worker_id) {
        cluster_ipc::wait_channel_eof(worker_id, std::time::Duration::from_millis(300));
    }
    drain_ipc_inbox(vm)?;
    // Node `worker.process.once('exit')`：`if (!worker.isConnected()) removeWorker(worker)`
    // ——本运行时此处通道已断（上一行刚派发过 `'disconnect'`，`isConnected()` 为
    // false），且子进程既已退出就不可能再持有活通道，故等价地**总是**移除：
    // 出表 + （表空时）`intercom.emit('disconnect')`——primary 发起的
    // `cluster.disconnect(cb)` 的 cb 正由此触发（落在 `'disconnect'` 之后、
    // `'exit'` 之前）。
    emit_disconnect(vm, worker_ref)?;
    remove_worker(vm, worker_ref)?;
    // child 'exit'(code, signal)：code 缺失（信号终止等）时按 Node 的 null 语义
    // 传 null；signal 本运行时无法区分，恒 null。
    let code = args.first().copied().unwrap_or(Value::Null);
    set_phase(vm, worker_ref, WorkerPhase::Dead);
    // IPC 通道随进程退出关闭：停掉 accept/读线程并令后续 send 返回 false。
    cluster_ipc::close_channel(worker_id);
    let worker_val = Value::Object(ObjectRef(worker_ref));
    ns_emit(vm, module_val, "exit", &[worker_val, code, Value::Null])?;
    ns_emit(vm, worker_val, "exit", &[code, Value::Null])?;
    Ok(Value::Undefined)
}

/// 派发 `'disconnect'`（Node `primary.js:191-211` 的 `worker.process.once('disconnect')`）：
/// `exitedAfterDisconnect = !!exitedAfterDisconnect` → `state = 'disconnected'` →
/// `worker.emit('disconnect')`（**无实参**）→ `cluster.emit('disconnect', worker)`
/// （**1 实参**）。worker 仍在 `workers` 表中（Node：仅 `isDead()` 时才移除，
/// 而此刻进程未退，故保留——实测 oracle `workers=1`）。
///
/// 幂等：已 `Disconnected`/`Dead` 的 worker 直接返回（`'disconnect'` 在 Node 为 once）。
fn emit_disconnect(vm: &mut Vm, worker_ref: u32) -> Result<(), VmError> {
    match phase_of(worker_ref) {
        Some(WorkerPhase::Disconnected | WorkerPhase::Dead) | None => return Ok(()),
        _ => {}
    }
    set_phase(vm, worker_ref, WorkerPhase::Disconnected);
    let worker_val = Value::Object(ObjectRef(worker_ref));
    // Node 在 disconnect / exit 两处都做 `= !!值` 归一：初值 `undefined` → `false`；
    // primary 侧 `Worker.prototype.disconnect()` 先置的 `true` 必须保持
    // （实测 p7：primary 发起断连后 `'disconnect'` 处 `ead=true`）。
    let expected = matches!(
        vm.get_property(worker_val, "exitedAfterDisconnect"),
        Ok(Value::Boolean(true))
    );
    let _ = vm.set_property(
        worker_val,
        "exitedAfterDisconnect",
        Value::Boolean(expected),
    );
    ns_emit(vm, worker_val, "disconnect", &[])?;
    if let Some(module_ref) = vm.builtin_registry.module("cluster") {
        ns_emit(vm, Value::Object(module_ref), "disconnect", &[worker_val])?;
    }
    Ok(())
}

/// workers 对象的键数（worker id 个数）。
fn workers_count(vm: &mut Vm, module_val: Value) -> usize {
    let Ok(workers_val) = vm.get_property(module_val, "workers") else {
        return 0;
    };
    vm.own_properties(workers_val).len()
}

// ---------------------------------------------------------------------------
// primary 侧：intercom（`internal/cluster/primary.js` 的 intercom EventEmitter）、
// `removeWorker` 与 primary `Worker.prototype.disconnect()`
// ---------------------------------------------------------------------------

// `intercom.once('disconnect', cb)` 的待发回调（`cluster.disconnect(cb)` 注册）。
// 线程局部：堆值仅本线程 Vm 有效（GC 根见本模块 `store_roots`）。
thread_local! {
    static INTERCOM_ONCE: RefCell<Option<Vec<Value>>> = const { RefCell::new(None) };
}

/// 注册 `intercom.once('disconnect', cb)`。
fn intercom_once_disconnect(cb: Value) {
    INTERCOM_ONCE.with(|g| g.borrow_mut().get_or_insert_with(Vec::new).push(cb));
}

/// `intercom.emit('disconnect')`：取走全部 once 监听器后以 **0 实参**逐个调用
/// （Node EventEmitter 无实参发射 → cb 的 `arguments.length === 0`，实测 p7）。
///
/// **先取后调**：既是 `once` 语义（Node 在调用前摘除），也保证回调内再次
/// `cluster.disconnect(cb)` 注册的新监听器不会被本次 emit 消费。
fn emit_intercom_disconnect(vm: &mut Vm) -> Result<(), VmError> {
    let cbs = INTERCOM_ONCE
        .with(|g| std::mem::take(&mut *g.borrow_mut()))
        .unwrap_or_default();
    for cb in cbs {
        vm.invoke_callable(cb, Value::Undefined, &[])?;
    }
    Ok(())
}

/// `cluster.disconnect()` 在 `workers` 为空时的 nextTick 载体
/// （Node `process.nextTick(() => intercom.emit('disconnect'))`）。
fn intercom_disconnect_nt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    emit_intercom_disconnect(vm)?;
    Ok(Value::Undefined)
}

/// Node `internal/cluster/primary.js:142-150` 的 `removeWorker`：
/// `delete cluster.workers[worker.id]`，**表空**时 `intercom.emit('disconnect')`
/// ——这正是 `cluster.disconnect(cb)` 的 cb 触发点（实测 p7：cb 在 `'disconnect'`
/// 之后、`'exit'` 之前以 0 实参触发，且此刻 `workers` 已为 0）。
///
/// `handles` 为空故 Node 的 `assert(handles.size === 0)` 无需对应实现（本运行时
/// 无句柄传递）。
fn remove_worker(vm: &mut Vm, worker_ref: u32) -> Result<(), VmError> {
    let Some(module_ref) = vm.builtin_registry.module("cluster") else {
        return Ok(());
    };
    let module_val = Value::Object(module_ref);
    if let Ok(id) = vm.get_property(Value::Object(ObjectRef(worker_ref)), "id") {
        let key = vm.format_value(id);
        let workers_val = vm.get_property(module_val, "workers")?;
        if matches!(workers_val, Value::Object(_)) {
            vm.delete_property(workers_val, &key);
        }
    }
    if workers_count(vm, module_val) == 0 {
        emit_intercom_disconnect(vm)?;
    }
    Ok(())
}

/// primary 侧 `Worker.prototype.disconnect()`（Node `primary.js:360-366`）：
/// `exitedAfterDisconnect = true` → 发 `{"t":"d"}` 帧 → `removeHandlesForWorker`
/// （本运行时无句柄传递，无对应动作）→ `removeWorker`（**立即**出表）→ `return this`。
///
/// worker 侧收到 `d` 帧后走 `_disconnect(true)`：关闭本进程内监听中的 server →
/// `process.disconnect()`（通道 EOF 后 primary 派发 `'disconnect'`）。
fn primary_worker_disconnect(vm: &mut Vm, worker_ref: u32) -> Result<Value, VmError> {
    let worker_val = Value::Object(ObjectRef(worker_ref));
    let _ = vm.set_property(worker_val, "exitedAfterDisconnect", Value::Boolean(true));
    if let Ok(id_val) = vm.get_property(worker_val, "id") {
        let worker_id = crate::ops::to_number(id_val) as u64;
        let _ = cluster_ipc::send_to_worker(worker_id, &format!("{FRAME_DISCONNECT}\n"));
    }
    remove_worker(vm, worker_ref)?;
    Ok(worker_val)
}

/// primary 侧 `worker.disconnect()` 的方法面（`this` = Worker 包装对象）。
fn worker_disconnect(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    match current_receiver() {
        Value::Object(r) => primary_worker_disconnect(vm, r.0),
        other => Ok(other),
    }
}

// ---------------------------------------------------------------------------
// settings（setupMaster / setupPrimary）与 fork 的取值契约
// ---------------------------------------------------------------------------

/// `cluster.setupMaster([settings])` / `setupPrimary([settings])`。
///
/// Node 语义（源码 `internal/cluster/primary.js` `setupPrimary`，本机
/// v22.22.2 实测核对）：
/// ```text
/// settings = { args: process.argv.slice(2), exec: process.argv[1],
///              execArgv: process.execArgv, silent: false,
///              ...cluster.settings, ...options }
/// cluster.settings = settings;      // 每次**重建对象**（非原地改写）
/// ```
/// 要点：① 默认值每次都写入；② 旧 settings 覆盖默认值；③ options（**含未知键**）
/// 覆盖前两者；④ fork 会先隐式调用本函数，故裸 `fork()` 亦会填充默认值。
fn cluster_setup_master(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let opts = args.first().copied();

    let merged = vm.alloc_ordinary();

    // ① 默认值。exec 缺省为「当前主脚本的绝对路径」（Node 的 `process.argv[1]`
    //    由 Node 自身解析为绝对路径；本运行时同样绝对化以便 settings.cwd 生效）。
    //    主脚本缺失（无 entry_file 且 argv 无脚本段）时**不写入 exec 键**——
    //    与 Node 的 `exec: undefined` 同形，交由 fork 的 validator 报错。
    let script = current_script(vm);
    if !script.is_empty() {
        let exec_ref = vm.alloc_string(script);
        set_own(vm, merged, "exec", Value::Object(exec_ref));
    }
    let argv_default = cli_args(vm);
    let argv_ref = vm.alloc_array(argv_default);
    set_own(vm, merged, "args", Value::Object(argv_ref));
    // execArgv：Node 缺省 `process.execArgv`。本运行时无「node 旗标」槽位
    // （进程形态为 `aluka run <script>`），恒为空数组（登记偏离）。
    let exec_argv_ref = vm.alloc_array(Vec::new());
    set_own(vm, merged, "execArgv", Value::Object(exec_argv_ref));
    set_own(vm, merged, "silent", Value::Boolean(false));

    // ② 旧 settings 覆盖默认值。
    if let Ok(Value::Object(prev)) = vm.get_property(self_val, "settings") {
        for (k, v) in vm.own_properties(Value::Object(prev)) {
            set_own(vm, merged, &k, v);
        }
    }
    // ③ options 浅合并覆盖（Node 用对象展开，未知键同样保留、undefined 同样覆盖）。
    if let Some(Value::Object(_)) = opts {
        for (k, v) in vm.own_properties(opts.unwrap_or(Value::Undefined)) {
            set_own(vm, merged, &k, v);
        }
    }

    vm.set_property(self_val, "settings", Value::Object(merged))?;
    Ok(Value::Undefined)
}

/// 忽略失败的 `set_property`（settings 对象的键均为自有数据属性）。
fn set_own(vm: &mut Vm, obj: ObjectRef, key: &str, val: Value) {
    let _ = vm.set_property(Value::Object(obj), key, val);
}

/// 当前主脚本的**绝对**路径（Node `process.argv[1]` 的对应物）。
///
/// 本运行时 `process.argv` 无 exe 槽位（`[script, ...cli]`），故主脚本取
/// `entry_file`（字节码模式下为 .bc 路径——`aluvm run app.bc` 的 argv 首段是
/// 子命令 `run`，需先过滤），回退命令行首段。相对路径按主进程 cwd 绝对化，
/// 与 Node 一致：Node 的 `process.argv[1]` 亦为解析后的绝对路径。
fn current_script(vm: &Vm) -> String {
    let raw = if vm.entry_file.is_empty() {
        std::env::args()
            .nth(1)
            .filter(|a| !matches!(a.as_str(), "run" | "test" | "-v" | "--version"))
            .unwrap_or_default()
    } else {
        vm.entry_file.clone()
    };
    if raw.is_empty() {
        return raw;
    }
    let path = std::path::Path::new(&raw);
    if path.is_absolute() {
        return raw;
    }
    std::env::current_dir()
        .map(|d| d.join(path).display().to_string())
        .unwrap_or(raw)
}

/// 主进程的额外 CLI 参数（Node `settings.args` 缺省 = `process.argv.slice(2)`）。
///
/// 本运行时 argv 比 Node 少一个 exe 槽位 → 等价切片为 `slice(1)`。
fn cli_args(vm: &mut Vm) -> Vec<Value> {
    let Some(p) = vm.process_object else {
        return Vec::new();
    };
    let Ok(Value::Object(arr)) = vm.get_property(Value::Object(p), "argv") else {
        return Vec::new();
    };
    let Some(HeapObject::Array { elements, .. }) = vm.heap.get(arr.0 as usize) else {
        return Vec::new();
    };
    elements.iter().skip(1).copied().collect()
}

/// `settings.exec` → spawn 的 modulePath。Node `child_process.fork` 的 validator
/// 文本：须为 string（本运行时不含 Buffer/URL 形态，登记偏离），否则 TypeError。
fn settings_exec(vm: &mut Vm, settings: Value) -> Result<String, VmError> {
    let v = vm
        .get_property(settings, "exec")
        .unwrap_or(Value::Undefined);
    if let Some(s) = heap_string(vm, v) {
        return Ok(s);
    }
    Err(invalid_arg_type_throw(
        vm,
        &format!(
            "The \"modulePath\" argument must be of type string or an instance of \
             Buffer or URL. Received {}",
            received_repr(vm, v)
        ),
    ))
}

/// `settings.args` → spawn 的实参串。
///
/// Node 侧规则（`lib/child_process.js::fork`，v22.22.2 实测核对）：
/// `null`/`undefined` → `[]`；数组 → 逐元素取用；**非数组对象 → 被当作 fork 的
/// options 参数（args 置空）**；其余原始类型 → `validateArray` 抛 TypeError。
/// 本运行时对口径：前三者按 Node 实现，**非数组对象按「args 置空」处理但忽略其
/// options 语义**（登记偏离——Node 会用它覆盖 `cwd`/`silent`/`stdio` 等）。
fn settings_args(vm: &mut Vm, settings: Value) -> Result<Vec<String>, VmError> {
    let v = vm
        .get_property(settings, "args")
        .unwrap_or(Value::Undefined);
    if matches!(v, Value::Undefined | Value::Null) {
        return Ok(Vec::new());
    }
    let Value::Object(r) = v else {
        return Err(args_type_error(vm, v));
    };
    match vm.heap.get(r.0 as usize) {
        Some(HeapObject::Array { elements, .. }) => {
            Ok(elements.iter().map(|e| vm.format_value(*e)).collect())
        }
        // 堆上的原始类型（字符串/BigInt/Symbol）在 JS 侧 `typeof !== 'object'`，
        // 同 Node 走 validateArray 抛错——不可并入下面的「非数组对象」分支。
        Some(HeapObject::String(_) | HeapObject::BigInt(_) | HeapObject::Symbol { .. }) => {
            Err(args_type_error(vm, v))
        }
        // 真对象（Ordinary/Map/Date/…）：Node 视作 fork options（args 置空）。
        Some(_) => Ok(Vec::new()),
        None => Err(args_type_error(vm, v)),
    }
}

/// `args` 非数组原始值的 TypeError 文本。
fn args_type_error(vm: &mut Vm, v: Value) -> VmError {
    invalid_arg_type_throw(
        vm,
        &format!(
            "The \"args\" argument must be an instance of Array. Received {}",
            received_repr(vm, v)
        ),
    )
}

/// `settings.silent`（Node 传入 `child_process.fork` 的 silent；缺省 false）。
/// 真值 → 管道 stdio（不继承），假值/缺省 → 继承。
fn read_silent(vm: &mut Vm, settings: Value) -> bool {
    matches!(
        vm.get_property(settings, "silent"),
        Ok(Value::Boolean(true))
    )
}

/// `settings.cwd`（Node 传入 `child_process.fork` 的 cwd；缺省 undefined →
/// 继承主进程 cwd）。空串/非字符串一律按「继承」处理（Node 侧由 spawn 校验，
/// 本运行时取宽松口径并登记）。
fn read_cwd(vm: &mut Vm, settings: Value) -> String {
    match vm.get_property(settings, "cwd") {
        Ok(v) => heap_string(vm, v).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// 取字符串堆对象的内部值（非字符串返回 None）。
fn heap_string(vm: &Vm, v: Value) -> Option<String> {
    let Value::Object(r) = v else {
        return None;
    };
    match vm.heap.get(r.0 as usize) {
        Some(HeapObject::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Node validator 文本里 `Received …` 的取值描述。
///
/// 实测核对（Node v22.22.2）：`undefined` / `null` 无 `type` 前缀；原始类型为
/// `type <typeof> (<inspect 值>)`；数组与一般对象为 `an instance of Array|Object`。
/// **未覆盖**：函数（Node 作 `Received function <name>`）与其它 exotic 形（登记偏离）。
fn received_repr(vm: &Vm, v: Value) -> String {
    match v {
        Value::Undefined => "undefined".to_owned(),
        Value::Null => "null".to_owned(),
        Value::Boolean(b) => format!("type boolean ({b})"),
        Value::Number(n) => format!("type number ({})", vm.format_value(Value::Number(n))),
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::String(s)) => format!("type string ('{s}')"),
            Some(HeapObject::BigInt(s)) => format!("type bigint ({s}n)"),
            Some(HeapObject::Symbol { description, .. }) => {
                format!(
                    "type symbol ({})",
                    crate::symbol::symbol_display(description)
                )
            }
            Some(HeapObject::Array { .. }) => "an instance of Array".to_owned(),
            _ => "an instance of Object".to_owned(),
        },
    }
}

/// 构造 Node 形态的参数校验 TypeError（`code=ERR_INVALID_ARG_TYPE`）并抛出。
fn invalid_arg_type_throw(vm: &mut Vm, msg: &str) -> VmError {
    let obj = vm.alloc_error_instance(msg);
    let name_ref = vm.alloc_string("TypeError".to_owned());
    let code_ref = vm.alloc_string("ERR_INVALID_ARG_TYPE".to_owned());
    let recv = Value::Object(obj);
    let _ = vm.set_property(recv, "name", Value::Object(name_ref));
    let _ = vm.set_property(recv, "code", Value::Object(code_ref));
    VmError::Thrown(recv)
}

/// `cluster.disconnect([callback])`（Node `internal/cluster/primary.js:223-238`）：
///
/// ```text
/// const workers = ObjectValues(cluster.workers);
/// if (workers.length === 0) {
///   process.nextTick(() => intercom.emit('disconnect'));
/// } else {
///   for (const worker of workers) {
///     if (worker.isConnected()) worker.disconnect();   // 发 d 帧 + 立即出表
///   }
/// }
/// if (typeof cb === 'function') intercom.once('disconnect', cb);   // 循环之后
/// ```
///
/// 要点（实测基线 v22.23.1，探针 p7）：
/// ① 每个 `isConnected()` 的 worker 走 primary `Worker.prototype.disconnect()`，
///    **同步**清空 `workers` 表并返回 `undefined`（`workers-after=0`）；
/// ② `cb` 在循环**之后**才注册，故循环内 `removeWorker` 的那次
///    `intercom.emit('disconnect')` 不被该 cb 消费——cb 实际由**退出转接**里的
///    `removeWorker`（表已空）触发，落在 `'disconnect'` 之后、`'exit'` 之前；
/// ③ 表本来就空时走 `process.nextTick(...)` 异步触发（Node 同序）；
/// ④ 函数**无 return**（→ `undefined`）。
fn cluster_disconnect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let workers_val = vm.get_property(self_val, "workers")?;
    let worker_vals: Vec<Value> = vm
        .own_properties(workers_val)
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    if worker_vals.is_empty() {
        // Node：`process.nextTick(() => intercom.emit('disconnect'))`。
        let nt = vm.alloc_native_fn("cluster.__intercomDisconnectNT");
        vm.nexttick_queue.push_back(Value::Object(nt));
    } else {
        for w in worker_vals {
            let Value::Object(r) = w else {
                continue;
            };
            // `worker.isConnected()`：Node 为 `this.process.connected`（通道连通性）。
            if phase_connected(phase_of(r.0)) {
                primary_worker_disconnect(vm, r.0)?;
            }
        }
    }
    if let Some(cb) = args.first().copied() {
        if is_callable(vm, cb) {
            intercom_once_disconnect(cb);
        }
    }
    Ok(Value::Undefined)
}

/// `cluster.Worker` 构造器：返回普通对象（Go 同款表面）。
fn cluster_worker_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(vm.alloc_ordinary()))
}

/// primary → worker 方向的消息帧（`t: m`，与 worker → primary 同一载荷语义）。
fn send_worker_message(vm: &mut Vm, worker_id: u64, msg: Value) -> Result<bool, VmError> {
    let line = frame_message(vm, msg)?;
    Ok(cluster_ipc::send_to_worker(worker_id, &line))
}

/// worker `send(message)`：经 IPC 通道写入 primary（返回通道真实可写性，
/// Node `Worker.send` 在通道关闭/进程已退出时返回 false）。
fn worker_send(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if !matches!(receiver, Value::Object(_)) {
        return Ok(Value::Boolean(false));
    }
    // worker id：Worker 包装对象自带 `id` 属性。
    let Ok(id_val) = vm.get_property(receiver, "id") else {
        return Ok(Value::Boolean(false));
    };
    let worker_id = crate::ops::to_number(id_val) as u64;
    let Some(msg) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(send_worker_message(vm, worker_id, msg)?))
}

/// worker `kill()`：简化恒 true（Go 同款）。
fn worker_kill(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    worker_destroy(vm, _args)?;
    Ok(Value::Boolean(true))
}

/// worker `destroy()`。
fn worker_destroy(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    // M5.2：destroy 终止底层子进程（Node 语义：disconnect 关闭全部 worker）；
    // 委托 child 对象的 kill 方法（退出事件经既有转接派发）
    let receiver = current_receiver();
    if let Ok(child) = vm.get_property(receiver, "process") {
        if let Ok(kill) = vm.get_property(child, "kill") {
            if matches!(kill, Value::Object(_)) {
                let _ = vm.invoke_callable(kill, child, &[]);
            }
        }
    }
    Ok(Value::Undefined)
}

/// worker `isConnected()`：IPC 通道连通（Node `state === 'online'`）。
fn worker_is_connected(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(phase_connected(current_worker_phase())))
}

/// worker `isDead()`：进程已退出（Node `state === 'dead'`）。
fn worker_is_dead(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(phase_dead(current_worker_phase())))
}

/// 当前接收者的生命周期状态（`this` 为 Worker 包装对象）。
fn current_worker_phase() -> Option<WorkerPhase> {
    match current_receiver() {
        Value::Object(r) => phase_of(r.0),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// process 面（worker 进程内：process.send / process.connected）
// ---------------------------------------------------------------------------

/// worker 进程启动时建立 IPC 通道、**激活 IPC 事件源**并上报 `'online'`；
/// 无通道环境变量/连接失败返回 false（Node 中 `process.send` 不存在，与
/// `silent`/无 ipc 的子进程一致）。
///
/// Node 在 bootstrap 阶段建立通道，与是否 `require('cluster')` 无关——故调用点
/// 在解释器构建 process 对象处（见 `interpreter.rs`）。
///
/// **保活语义（M5.2 实测口径）**：Node 中 fork 出的子进程 IPC 通道**默认保活**
/// （脚本跑完不退出，实测 14s 后仍 `connected=true`；`process.channel.unref()`
/// 才立即释放）。故本运行时在通道建立即激活 `cluster_ipc` 事件源；通道关闭
/// （对端 EOF 或 `process.disconnect()`）后由泵自行注销（见 `cluster_ipc_busy`）。
pub(crate) fn worker_setup_channel(vm: &mut Vm) -> bool {
    if !cluster_ipc::child_connect() {
        return false;
    }
    let _ = cluster_ipc::child_send_line(&format!("{FRAME_ONLINE}\n"));
    vm.activate_event_source("cluster_ipc", cluster_ipc_pump);
    true
}

/// worker 进程的 IPC 通道是否连通（`process.connected` 初值）。
pub(crate) fn worker_channel_connected() -> bool {
    cluster_ipc::child_is_connected()
}

// worker 自身对象句柄（`cluster.worker`；仅 worker 进程内有值）。
thread_local! {
    static WORKER_SELF: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// 记录 worker 自身对象句柄（`build` 在 worker 进程内调用）。
fn set_worker_self(id: u32) {
    WORKER_SELF.with(|c| c.set(Some(id)));
}

/// worker 自身对象的值（无 `cluster.worker` 时 `None`）。
fn worker_self_value() -> Option<Value> {
    WORKER_SELF
        .with(|c| c.get())
        .map(|id| Value::Object(ObjectRef(id)))
}

/// worker 进程内 `process` 单例句柄（事件派发目标与监听器存储键）。
fn process_value(vm: &Vm) -> Option<Value> {
    vm.process_object.map(Value::Object)
}

/// 当前进程是否为 cluster worker（`ALUKA_WORKER_ID` 由 `fork` 注入）。
fn is_worker_process() -> bool {
    std::env::var_os("ALUKA_WORKER_ID").is_some()
}

// ---------------------------------------------------------------------------
// worker 侧：process ↔ cluster.worker 桥接、`process.disconnect()` 与帧派发
// ---------------------------------------------------------------------------

thread_local! {
    /// worker 侧桥接是否已挂接（Node：`require('cluster')` 时一次性挂接）。
    static BRIDGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// worker 侧 `'disconnect'` 是否已派发（Node `process.once('disconnect')` 幂等）。
    static SELF_DISCONNECTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 首次 `require('cluster')`（worker 进程内）时挂接 process → cluster.worker 桥接。
///
/// Node 语义（`internal/cluster/child.js::_setupWorker` + `worker.js` 的 Worker
/// ctor）：
/// * `process.on('message', (m, h) => worker.emit('message', m, h))` —— 单向桥接；
/// * `process.once('disconnect', () => { worker.emit('disconnect');
///   if (!worker.exitedAfterDisconnect) process.exit(0); })` —— 非预期断连即退出。
///
/// 本运行时的 cluster 模块在 `Vm` 初始化阶段统一构建（无「首次 require」时点），
/// 故桥接在 `require('cluster')` 处按需挂接（调用点见 `modules.rs`）：这使
/// `process.listenerCount('message')` 的可见值与 Node 一致（require 后为 1）。
pub(crate) fn on_cluster_required(vm: &mut Vm, name: &str) {
    if name != "cluster" || !is_worker_process() || BRIDGED.with(|c| c.get()) {
        return;
    }
    let Some(proc) = vm.process_object else {
        return;
    };
    BRIDGED.with(|c| c.set(true));
    let bridge = vm.alloc_native_fn("cluster.__workerBridgeMessage");
    ns_push_listener(proc.0, "message", Value::Object(bridge));
    let disc = vm.alloc_native_fn("cluster.__workerBridgeDisconnect");
    ns_push_listener(proc.0, "disconnect", Value::Object(disc));
}

/// 桥接载体：把 process 的 `'message'` 转发给 `cluster.worker`（Node Worker ctor）。
fn worker_bridge_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(worker) = worker_self_value() {
        let emit_args: Vec<Value> = args.to_vec();
        ns_emit(vm, worker, "message", &emit_args)?;
    }
    Ok(Value::Undefined)
}

/// 桥接载体：`'disconnect'` 转发给 `cluster.worker`，非预期断连按 Node 立即退出
/// （`process.exit(0)`；`exitedAfterDisconnect` 为假值即视为非预期）。
fn worker_bridge_disconnect(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(worker) = worker_self_value() {
        ns_emit(vm, worker, "disconnect", &[])?;
        let expected = matches!(
            vm.get_property(worker, "exitedAfterDisconnect"),
            Ok(Value::Boolean(true))
        );
        if !expected {
            return crate::builtins::require_aliases::process_exit(vm, &[Value::Number(0.0)]);
        }
    }
    Ok(Value::Undefined)
}

/// `process.disconnect()`（worker 侧）：关闭 IPC 通道并（**异步**）派发 `'disconnect'`。
///
/// Node 语义（实测 v22.23.1）：返回 `undefined`；`process.connected` **同步**翻
/// `false`；再次调用抛 `ERR_IPC_DISCONNECTED`（文本
/// `IPC channel is already disconnected`）。`'disconnect'` 事件经 `nextTick`
/// 派发——实测 `process.disconnect()` 返回后调用栈内的后续语句**先**执行完
/// （Node 侧也仍能打出其后的 console.log），故不能在调用栈内同步发射。
fn process_disconnect(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if !cluster_ipc::child_close_channel() {
        let err = vm.alloc_error_instance("IPC channel is already disconnected");
        let err_val = Value::Object(err);
        let code = Value::Object(vm.alloc_string("ERR_IPC_DISCONNECTED".to_owned()));
        let _ = vm.set_property(err_val, "code", code);
        return Err(VmError::Thrown(err_val));
    }
    set_process_connected(vm, false);
    let nt = vm.alloc_native_fn("cluster.__selfDisconnectNT");
    vm.nexttick_queue.push_back(Value::Object(nt));
    Ok(Value::Undefined)
}

/// `process.disconnect()` 的 nextTick 载体（`this === undefined`，故无需实参）。
fn self_disconnect_nt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    emit_self_disconnect(vm)
}

/// worker 进程内 `cluster.worker.isConnected()`：通道连通性（Node `Worker`
/// 的 `this.process.connected`）。
fn worker_self_is_connected(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(proc) = process_value(vm) {
        if let Ok(Value::Boolean(b)) = vm.get_property(proc, "connected") {
            return Ok(Value::Boolean(b));
        }
    }
    Ok(Value::Boolean(cluster_ipc::child_is_connected()))
}

/// worker 进程内 `cluster.worker.isDead()`：Node 为
/// `this.process.exitCode != null || this.process.signalCode != null`——
/// 本运行时未接线 worker 侧 `exitCode`/`signalCode`，运行期恒 `false`（登记偏离）。
fn worker_self_is_dead(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// worker 侧 `cluster.worker.disconnect()`（Node `internal/cluster/child.js:287-294`）：
/// `state` 不是 `'disconnecting'`/`'destroying'` 时**同步**置 `state = 'disconnecting'`
/// 并执行 `_disconnect()`；**返回 `this`**（实测 p8：`ret-is-self=true`、
/// `ret-type=object`，且返回时 `state=disconnecting`、`ead=true`、`listening=false`）。
///
/// 重复调用（已在 `'disconnecting'`）按 Node 语义为 no-op，仍返回 `this`。
fn worker_self_disconnect(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if !matches!(receiver, Value::Object(_)) {
        return Ok(receiver);
    }
    let state = vm
        .get_property(receiver, "state")
        .unwrap_or(Value::Undefined);
    let name = vm.format_value(state);
    if name == "disconnecting" || name == "destroying" {
        return Ok(receiver);
    }
    let disconnecting = vm.alloc_string("disconnecting".to_owned());
    let _ = vm.set_property(receiver, "state", Value::Object(disconnecting));
    worker_self_disconnect_impl(vm, false)?;
    Ok(receiver)
}

/// Node `internal/cluster/child.js:253-284` 的 `_disconnect(primaryInitiated)`：
///
/// ```text
/// this.exitedAfterDisconnect = true;            // ① 同步
/// for (const handle of handles.values()) …close(cb);   // ② 关本进程内 server 句柄
/// // ③ 全部关完后：
/// if (primaryInitiated) process.disconnect();
/// else send({act: 'exitedAfterDisconnect'}, () => process.disconnect());
/// ```
///
/// ① 与 p8 的 `W after … ead=true` 对应（同一调用栈内可见）；
/// ② 本运行时按 net / http 两张线程局部表批量关闭并派发 `'close'`；
/// ③ `primaryInitiated` 时直接断连；worker 自发起时上报 `{"t":"e"}` 帧后
///    **挂起**——等 primary 的 ack 回程（同帧 `{"t":"e"}`）才收尾
///    `process.disconnect()`（Node 的 send 回调口径；可观测差异：`disconnect()`
///    同步返回后 `process.connected` 仍为 `true`，实测 oracle）。上报失败（通道
///    已断）则立即收尾，不等待。重复调用在挂起期间为 no-op。
///
/// 非 worker 进程（无 `cluster.worker`）调用时只关 server 并断连——与 Node 的
/// bootstrap 语义一致（Worker 对象始终存在，此处仅防御性处理）。
fn worker_self_disconnect_impl(vm: &mut Vm, primary_initiated: bool) -> Result<(), VmError> {
    if let Some(worker) = worker_self_value() {
        let _ = vm.set_property(worker, "exitedAfterDisconnect", Value::Boolean(true));
    }
    close_worker_servers(vm);
    if !primary_initiated {
        // 已有在途上报：挂起中，不重复发送（对齐 Node `_disconnect` 的
        // `disconnected` 守卫形态）
        if WORKER_ACK_PENDING.with(|c| c.get()) {
            return Ok(());
        }
        if cluster_ipc::child_send_line(&format!("{FRAME_EXITED_AFTER_DISCONNECT}\n")) {
            // ack 回程未到：收尾断连延后到 `dispatch_self_frame` 的 `e` 帧
            WORKER_ACK_PENDING.with(|c| c.set(true));
            return Ok(());
        }
    }
    process_disconnect(vm, &[])?;
    Ok(())
}

/// 关闭本 worker 进程内**全部监听中的** server（Node `_disconnect` 遍历 `handles`）。
///
/// net 与 http 各有一张线程局部服务器表（http 的监听 socket **不在**
/// `NET_SHARED.servers`），故两张表都要扫；各自的批量关闭函数负责派发 `'close'`。
fn close_worker_servers(vm: &mut Vm) {
    crate::builtins::net::close_all_servers(vm);
    crate::builtins::http::server::close_all_servers(vm);
}

// ---------------------------------------------------------------------------
// `process.channel`（worker 侧；M5.2）
// ---------------------------------------------------------------------------

// worker 进程内的 `process.channel` 单例句柄（仅通道建立时创建）。
thread_local! {
    static WORKER_CHANNEL: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// 取（首次调用时创建）`process.channel` 对象。
///
/// Node 侧该对象是 `internal/child_process` 的 `Control` 实例（EventEmitter 子类），
/// `Control.prototype` 上有 `refCounted`/`unrefCounted`/`ref`/`unref`/`fd` getter。
/// 本运行时只接线**可观测的开关面**：
/// * `ref()`/`unref()`：显式设置保活（返回值 `undefined`，Node 一致）；
/// * `refCounted()`/`unrefCounted()`：计数式开关（Node `#refs`/`#refExplicitlySet` 公式）；
/// * `fd`：**仅提供同名自有键**（值 `undefined`）——Node 返回 IPC 管道 fd（实测 3），
///   本运行时介质为回环 TCP，无 fd3 管道（登记偏离；`'fd' in process.channel` 两侧同为真）。
///
/// 未接线（登记偏离）：`constructor.name === 'Control'`、channel 自身的
/// EventEmitter 面（`on`/`once`/`emit`…）与 `_handle`。
pub(crate) fn worker_channel_object(vm: &mut Vm) -> ObjectRef {
    if let Some(id) = WORKER_CHANNEL.with(|c| c.get()) {
        return ObjectRef(id);
    }
    let obj = vm.alloc_ordinary();
    ns_attach(
        vm,
        obj,
        "cluster:channel",
        &["ref", "unref", "refCounted", "unrefCounted"],
    );
    let _ = vm.set_property(Value::Object(obj), "fd", Value::Undefined);
    WORKER_CHANNEL.with(|c| c.set(Some(obj.0)));
    obj
}

/// 按当前 ref 状态同步 worker 侧 IPC 事件源（保活/解除保活）。
fn sync_worker_ipc_source(vm: &mut Vm) {
    if !is_worker_process() {
        return;
    }
    if cluster_ipc::child_channel_refed() {
        vm.activate_event_source("cluster_ipc", cluster_ipc_pump);
    } else {
        vm.deactivate_event_source("cluster_ipc");
    }
}

/// `process.channel.unref()`：解除保活（worker 可在事件循环排空后自然退出）。
fn channel_unref(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    cluster_ipc::child_channel_unref();
    sync_worker_ipc_source(vm);
    Ok(Value::Undefined)
}

/// `process.channel.ref()`：恢复保活。
fn channel_ref(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    cluster_ipc::child_channel_ref();
    sync_worker_ipc_source(vm);
    Ok(Value::Undefined)
}

/// `process.channel.refCounted()`：计数式 ref。
fn channel_ref_counted(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    cluster_ipc::child_channel_ref_counted();
    sync_worker_ipc_source(vm);
    Ok(Value::Undefined)
}

/// `process.channel.unrefCounted()`：计数式 unref。
fn channel_unref_counted(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    cluster_ipc::child_channel_unref_counted();
    sync_worker_ipc_source(vm);
    Ok(Value::Undefined)
}

/// worker 侧派发 `'disconnect'`（幂等）：先置 `process.connected = false`，再在
/// `process` 上派发（桥接随后转发给 `cluster.worker` 并按 Node 退出）。
fn emit_self_disconnect(vm: &mut Vm) -> Result<Value, VmError> {
    if SELF_DISCONNECTED.with(|c| c.replace(true)) {
        return Ok(Value::Undefined);
    }
    set_process_connected(vm, false);
    if let Some(proc) = process_value(vm) {
        ns_emit(vm, proc, "disconnect", &[])?;
    }
    Ok(Value::Undefined)
}

/// worker 侧收到 primary 下发的帧（`key == 0`）。
///
/// * `m`（message）：Node `internal/cluster/child.js` 把非内部协议帧直接
///   `process.emit('message', message, handle)`（本运行时无句柄传递，`handle` 恒
///   `undefined`）；`cluster.worker` 的 `'message'` 由桥接转发；
/// * `d`（disconnect，primary 发起）：Node `child.js` 的 `onmessage` 对
///   `{act:'disconnect'}` 调 `_disconnect(worker, true)`——**不**改
///   `cluster.worker.state`（`'disconnecting'` 只由 worker 自发起时写入），
///   置 `exitedAfterDisconnect = true`、关本进程内 server 后直接 `process.disconnect()`；
/// * `e`（自发起断连的 **ack 回程**）：worker 此前上报 `{"t":"e"}` 后挂起中——
///   收到即收尾 `process.disconnect()`（Node `send(..., () => process.disconnect())`
///   口径）。
fn dispatch_self_frame(vm: &mut Vm, text: &str) -> Result<(), VmError> {
    let Some((kind, frame)) = parse_frame(vm, text)? else {
        return Ok(());
    };
    match kind.as_str() {
        "d" => {
            worker_self_disconnect_impl(vm, true)?;
            return Ok(());
        }
        "e" => {
            if WORKER_ACK_PENDING.with(|c| c.replace(false)) {
                process_disconnect(vm, &[])?;
            }
            return Ok(());
        }
        "m" => {}
        _ => return Ok(()),
    }
    let Ok(msg) = vm.get_property(frame, "v") else {
        return Ok(());
    };
    if let Some(proc) = process_value(vm) {
        ns_emit(vm, proc, "message", &[msg, Value::Undefined])?;
    }
    Ok(())
}

/// worker 侧通道关闭（对端 EOF 或本地 `process.disconnect()`）：派发 `'disconnect'`。
fn worker_channel_closed(vm: &mut Vm) -> Result<(), VmError> {
    emit_self_disconnect(vm)?;
    Ok(())
}

/// 服务端 `listen` 成功后（worker 进程内）上报 `listening` 帧，并置 worker 侧
/// `cluster.worker.state = 'listening'`。
///
/// Node 语义（`internal/cluster/child.js:117-127`）：`obj.once('listening')` 里
/// 复用 queryServer 的 message（含 `address/addressType/fd`），把 `act` 改为
/// `'listening'`、`port` 改为 `obj.address()?.port || options.port` 后发出。
/// 本运行时在**绑定成功点**直接发出（不再依赖 worker 侧 `'listening'` 事件的
/// 注册顺序），primary 侧据此派发 `'listening'` 事件。
///
/// `address` 为 `None` 表示未指定 host（Node `message.address === null`，
/// 实测 oracle：`listen(0)` → `address=null`、`addressType=4`）。
/// 返回是否已上报（非 worker 进程 / 无 IPC 通道 → false）。
pub(crate) fn worker_notify_listening(
    vm: &mut Vm,
    address: Option<&str>,
    address_type: u8,
    port: u16,
) -> bool {
    if std::env::var_os("ALUKA_WORKER_ID").is_none() || !cluster_ipc::child_is_connected() {
        return false;
    }
    let address_json = match address {
        Some(a) => json_escape(a),
        None => "null".to_owned(),
    };
    // `fd` 不入帧：Node 侧 `message.fd` 为 undefined（JSON 序列化下键缺失），
    // primary 构造 `info` 时按语义补 `fd: undefined`。
    let line = format!(
        "{{\"t\":\"l\",\"addressType\":{address_type},\"address\":{address_json},\"port\":{port}}}\n"
    );
    if !cluster_ipc::child_send_line(&line) {
        return false;
    }
    // worker 侧 `cluster.worker.state = 'listening'`（cluster 模块未被 require
    // 时无 `cluster.worker`，Node 亦然——net.js 在 worker 内会 require cluster，
    // 本运行时仅当用户代码 require 过才存在，故为尽力而为）。
    if let Some(module_ref) = vm.builtin_registry.module("cluster") {
        let module_val = Value::Object(module_ref);
        if let Ok(worker_val) = vm.get_property(module_val, "worker") {
            if matches!(worker_val, Value::Object(_)) {
                if let Ok(state) = vm.get_property(worker_val, "state") {
                    if vm.format_value(state) == "online" {
                        let s = vm.alloc_string("listening".to_owned());
                        let _ = vm.set_property(worker_val, "state", Value::Object(s));
                    }
                }
            }
        }
    }
    true
}

/// 最小 JSON 字符串转义（地址串只含 IP/主机名，仍按规范处理控制字符）。
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `process.send(message[, callback])`：仅 worker 进程内存在。
///
/// 通道未建立/已关闭（含写失败）返回 false；提供了回调时按 Node 语义回调
/// `(err | null)`（写为同步落盘，回调在返回前调用）。
fn process_send(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(msg) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    // 回调位置：Node 为 `(message, sendHandle, callback)`，取尾部首个可调用值。
    let cb = args
        .iter()
        .skip(1)
        .rev()
        .copied()
        .find(|v| is_callable(vm, *v));
    if !cluster_ipc::child_is_connected() {
        notify_send_failure(vm, cb)?;
        return Ok(Value::Boolean(false));
    }
    let line = frame_message(vm, msg)?;
    if cluster_ipc::child_send_line(&line) {
        if let Some(cb) = cb {
            vm.invoke_callable(cb, Value::Undefined, &[Value::Null])?;
        }
        return Ok(Value::Boolean(true));
    }
    // 写失败：通道已断，同步翻转 process.connected。
    notify_send_failure(vm, cb)?;
    Ok(Value::Boolean(false))
}

/// 发送失败收尾：翻转 `process.connected` 并按 Node 语义回调错误。
fn notify_send_failure(vm: &mut Vm, cb: Option<Value>) -> Result<(), VmError> {
    set_process_connected(vm, false);
    if let Some(cb) = cb {
        let err = vm.alloc_error_instance("IPC channel is closed");
        vm.invoke_callable(cb, Value::Undefined, &[Value::Object(err)])?;
    }
    Ok(())
}

/// 写 `process.connected`（worker 进程内通道状态；primary 侧该属性不存在）。
fn set_process_connected(vm: &mut Vm, connected: bool) {
    if let Some(proc_ref) = vm.process_object {
        let _ = vm.set_property(
            Value::Object(proc_ref),
            "connected",
            Value::Boolean(connected),
        );
    }
}

// ---------------------------------------------------------------------------
// IPC 事件源（primary 侧：worker 帧 → 'online'/'message' 事件）
// ---------------------------------------------------------------------------

/// IPC 事件源泵：排空 inbox 中的帧并派发事件；不再需要保活时注销事件源
/// （否则事件循环会被闲置通道一直挂住）。
fn cluster_ipc_pump(vm: &mut Vm) -> Result<bool, VmError> {
    let progressed = drain_ipc_inbox(vm)?;
    if !cluster_ipc_busy() {
        vm.deactivate_event_source("cluster_ipc");
    }
    Ok(progressed)
}

/// 排空 inbox 并派发全部在途帧（事件源泵与 child `'exit'` 转接共用）。
///
/// 帧按通道 key 分派：`0` = 子进程自身通道（worker 侧收 primary 消息），
/// 其余 = 某 worker 的通道（primary 侧）。
fn drain_ipc_inbox(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    while let Some((key, item)) = cluster_ipc::take_incoming() {
        progressed = true;
        match item {
            cluster_ipc::Incoming::Line(text) if key == 0 => dispatch_self_frame(vm, &text)?,
            cluster_ipc::Incoming::Line(text) => dispatch_worker_frame(vm, key, &text)?,
            // 子进程自身通道关闭 = worker 与 primary 断开（Node `'disconnect'`）。
            cluster_ipc::Incoming::Closed if key == 0 => worker_channel_closed(vm)?,
            // 通道关闭 = worker 断开（Node 'disconnect'：进程未退出仍非 dead）。
            cluster_ipc::Incoming::Closed => mark_disconnected(vm, key)?,
        }
    }
    Ok(progressed)
}

/// 事件源活性。
///
/// * **worker 进程**：通道连通即保活（Node 实测：fork 出的子进程 IPC 通道默认
///   保活，脚本跑完不退出）；通道关闭后不再保活，事件循环可自然结束。
/// * **primary 进程**：还有非 Dead 的 worker。
/// * 两态都叠加「inbox 尚有未派发条目」，避免在途帧被搁浅。
fn cluster_ipc_busy() -> bool {
    let alive = if is_worker_process() {
        // worker 侧：通道连通**且**未被 `process.channel.unref()` 显式解除保活。
        cluster_ipc::child_is_connected() && cluster_ipc::child_channel_refed()
    } else {
        WORKER_PHASE.with(|g| {
            g.borrow()
                .as_ref()
                .is_some_and(|m| m.values().any(|p| !matches!(p, WorkerPhase::Dead)))
        })
    };
    alive || cluster_ipc::inbox_pending()
}

/// 通道关闭：派发 `'disconnect'`（已 Dead/Disconnected 的幂等不动）。
fn mark_disconnected(vm: &mut Vm, worker_id: u64) -> Result<(), VmError> {
    if let Some(worker_ref) = worker_ref_by_id(worker_id) {
        emit_disconnect(vm, worker_ref)?;
    }
    Ok(())
}

/// workers 表中某 id 对应的 worker 对象句柄 id（无此 worker 返回 None）。
fn worker_ref_by_id(worker_id: u64) -> Option<u32> {
    WORKER_BY_ID.with(|g| g.borrow().as_ref().and_then(|m| m.get(&worker_id).copied()))
}

/// 派发一条 worker 帧。
///
/// 帧类型（见 `cluster_ipc` 模块文档）：`o` = online、`m` = message、
/// `l` = listening、`e` = worker 自发起断连的先行上报（Node
/// `{act:'exitedAfterDisconnect'}`）。`{"t":"d"}` 是 primary → worker 方向，不在此列。
fn dispatch_worker_frame(vm: &mut Vm, worker_id: u64, text: &str) -> Result<(), VmError> {
    let Some((kind, frame)) = parse_frame(vm, text)? else {
        return Ok(());
    };
    let Some(worker_ref) = worker_ref_by_id(worker_id) else {
        return Ok(());
    };
    let worker_val = Value::Object(ObjectRef(worker_ref));
    let Some(module_ref) = vm.builtin_registry.module("cluster") else {
        return Ok(());
    };
    match kind.as_str() {
        "o" => {
            set_phase(vm, worker_ref, WorkerPhase::Online);
            ns_emit(vm, worker_val, "online", &[])?;
            ns_emit(vm, Value::Object(module_ref), "online", &[worker_val])?;
        }
        // Node `primary.js` 的 `listening(worker, message)`：payload 只取
        // `{addressType, address, port, fd}`（**键序即插入序**，`fd` 恒
        // `undefined` 但为自有键——实测 oracle `info-keys=addressType,address,port,fd`）。
        "l" => {
            let address_type = vm.get_property(frame, "addressType")?;
            let address = vm.get_property(frame, "address")?;
            let port = vm.get_property(frame, "port")?;
            let info = vm.alloc_ordinary();
            let info_val = Value::Object(info);
            let _ = vm.set_property(info_val, "addressType", address_type);
            let _ = vm.set_property(info_val, "address", address);
            let _ = vm.set_property(info_val, "port", port);
            let _ = vm.set_property(info_val, "fd", Value::Undefined);
            set_phase(vm, worker_ref, WorkerPhase::Listening);
            ns_emit(vm, worker_val, "listening", &[info_val])?;
            ns_emit(
                vm,
                Value::Object(module_ref),
                "listening",
                &[worker_val, info_val],
            )?;
        }
        // Node cluster 'message' 实参序为 (worker, message, handle)，worker 侧为
        // (message, handle)；无句柄传递故 handle 恒 undefined。
        "m" => {
            let Ok(msg) = vm.get_property(frame, "v") else {
                return Ok(());
            };
            ns_emit(vm, worker_val, "message", &[msg, Value::Undefined])?;
            ns_emit(
                vm,
                Value::Object(module_ref),
                "message",
                &[worker_val, msg, Value::Undefined],
            )?;
        }
        // Node `primary.js` 的 `exitedAfterDisconnect(worker, message)`：worker 自发起
        // 断连时先上报本帧，primary 置 `exitedAfterDisconnect = true`，随后回
        // ack（Node 回 `{ack: message.seq}`，本运行时帧无 seq，以同帧 `{"t":"e"}`
        // 回程）——worker 收到 ack 后才收尾 `process.disconnect()`（Node
        // `child.js` 的 `send(..., () => process.disconnect())` 口径）。仅有此帧
        // 先于 EOF 到达，`'disconnect'` 处才观测到 `ead=true`（实测 p8）。
        "e" => {
            let _ = vm.set_property(worker_val, "exitedAfterDisconnect", Value::Boolean(true));
            cluster_ipc::send_to_worker(worker_id, &format!("{FRAME_EXITED_AFTER_DISCONNECT}\n"));
        }
        _ => {}
    }
    Ok(())
}
fn is_callable(vm: &Vm, val: Value) -> bool {
    let Value::Object(r) = val else {
        return false;
    };
    matches!(
        vm.heap.get(r.0 as usize),
        Some(
            HeapObject::Closure { .. }
                | HeapObject::NativeFn { .. }
                | HeapObject::NativeCtor { .. }
        )
    )
}

// worker id → worker 对象句柄 id（IPC 帧按 id 定位 Worker；线程局部，堆句柄
// 仅本线程 Vm 有效）。
thread_local! {
    static WORKER_BY_ID: RefCell<Option<HashMap<u64, u32>>> = const { RefCell::new(None) };
}

thread_local! {
    /// worker 自发起断连的 ack 挂起标记：`{"t":"e"}` 上报已发出、等 primary
    /// 回程期间为真——收到回程帧才收尾 `process.disconnect()`（Node `send`
    /// 回调口径）。对端 EOF（primary 先死）时本标记不再消费：通道关闭路径
    /// 自会派发 `'disconnect'`，挂起即失效。
    static WORKER_ACK_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// 登记 worker id → 对象句柄。
fn register_worker_id(worker_id: u64, worker_ref: u32) {
    WORKER_BY_ID.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(worker_id, worker_ref);
    });
}

/// GC 根：`intercom.once('disconnect', cb)` 的待发回调（静态表持有堆值，
/// 漏登记即悬垂——见 `gc.rs::static_roots` 的纪律）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    INTERCOM_ONCE.with(|g| {
        if let Some(cbs) = g.borrow().as_ref() {
            for cb in cbs {
                out.push(*cb);
            }
        }
    });
}
