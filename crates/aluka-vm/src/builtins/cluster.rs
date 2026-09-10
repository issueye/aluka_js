//! `cluster` 内置模块（Phase 6 / M5.2 IPC 面）。
//!
//! 语义逐字对齐 Node.js 22 LTS 规范（实测基线 v22.23.1）：
//! - 模块对象自带事件器表面（`on/once/emit/...`，`'fork'/'online'/'exit'/'message'`）；
//! - `isPrimary`/`isMaster`/`isWorker`：环境变量 `ALUKA_WORKER_ID` 标记 worker
//!   进程（worker 进程内另有 `worker = {id, send}`）；
//! - `workers`（id → Worker 实例）、`settings`（setupMaster/setupPrimary 写入）、
//!   `schedulingPolicy`/`SCHED_NONE`(1)/`SCHED_RR`(2)；
//! - `fork()`：复用 `child_process.fork` 派生当前可执行文件重跑当前脚本
//!   （Go 用 `os.Args[1]` 作脚本路径），并包一层 Worker 对象：child 的
//!   `'exit'` 事件转接到 Worker 与 cluster（携带**真实退出码**）；
//! - **IPC 面（M5.2）**：`cluster_ipc` 承载 Node 的 ipc 管道语义（见该模块
//!   文档）——worker 侧 `process.send` / `process.connected` / `cluster.worker.send`
//!   （Node bootstrap 阶段建立通道，与是否 require 本模块无关），primary 侧
//!   `worker.send`（写端真实可写性）、`'online'`（worker 通道建立后上报）、
//!   `'message'`（`(worker, message, handle)` 实参序）；
//! - `disconnect([callback])`：对所有 worker 调 `destroy` 后调用回调；
//!   `Worker` 构造器返回普通对象（供 instanceof 表面）。
//!
//! 如实登记的缺口（Node 22 行为未覆盖）：
//! - **`NODE_UNIQUE_ID`：不实现**——实测 Node 22 的 cluster worker 内
//!   `process.env.NODE_UNIQUE_ID` 为 `undefined`（旧评审把它列为缺失项是误判）；
//! - worker 侧不激活 IPC 事件源：Node 子进程 channel 不阻止进程退出（本
//!   运行时的事件源无 unref 语义，激活会让 worker 无法自行退出），因此
//!   worker 侧收 primary 消息（`process.on('message')` 目前亦为空实现）与
//!   primary 侧 `disconnect()` 后 worker 内 `process.connected` 翻转均未落地；
//! - `listening` 事件、句柄（sendHandle）传递、`serialization: 'advanced'`。

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
use std::cell::RefCell;
use std::collections::HashMap;

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

/// Worker 生命周期状态（Node master 侧 `worker.state`，取值面收敛到本模块
/// 需要判定 `isConnected()`/`isDead()` 的子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerPhase {
    /// `'online'`：通道已建立（实测 Node 22 fork 返回时即为 online）
    Online,
    /// `'disconnected'`：IPC 通道已关闭但进程未退出
    Disconnected,
    /// `'dead'`：进程已退出
    Dead,
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

/// 置某 worker 对象的状态。
fn set_phase(worker_ref: u32, phase: WorkerPhase) {
    with_phase_map(|m| {
        m.insert(worker_ref, phase);
    });
}

/// 读取某 worker 对象的状态（未知句柄视为未登记 → `None`）。
fn phase_of(worker_ref: u32) -> Option<WorkerPhase> {
    WORKER_PHASE.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&worker_ref).copied())
    })
}

/// `worker.isConnected()` 的真实判定：仅 `'online'` 为 true（Node
/// `state === 'online' || state === 'listening'`；`listening` 本模块未实现）。
fn phase_connected(phase: Option<WorkerPhase>) -> bool {
    matches!(phase, Some(WorkerPhase::Online))
}

/// `worker.isDead()` 的真实判定（Node `state === 'dead'`）。
fn phase_dead(phase: Option<WorkerPhase>) -> bool {
    matches!(phase, Some(WorkerPhase::Dead))
}

/// IPC 帧：worker 通道建立后上报 `'online'`（Node 子进程 setupChannel 阶段）。
const FRAME_ONLINE: &str = "{\"t\":\"o\"}";

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

    // worker 进程内：cluster.worker = {id, send}——`send` 与 `process.send`
    // 同一条通道（Node `cluster.worker.send === fork 侧 process.send` 语义）。
    if !is_primary {
        let worker_obj = vm.alloc_ordinary();
        let id: f64 = worker_id_env.parse().unwrap_or(1.0);
        let _ = vm.set_property(Value::Object(worker_obj), "id", Value::Number(id));
        ns_attach(vm, worker_obj, "cluster:worker-self", &["send"]);
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
    // `process.send`（仅 worker 进程内挂属性，primary 侧为 undefined）：方法
    // 名经 NativeFn 全名 "process.send" 命中分派表，与是否 require cluster 无关，
    // 故在此模块（必定构建）登记。
    register_handler(registry, "process", "send", process_send);
    // worker 进程内 `cluster.worker.send`（同名空间只挂 send）。
    register_handler(registry, "cluster:worker-self", "send", process_send);
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
    Ok(obj)
}

/// `cluster.fork()`：child_process.fork 当前脚本 + Worker 包装。
fn cluster_fork(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    if !matches!(self_val, Value::Object(_)) {
        return Ok(self_val);
    }

    // worker id = 现有 workers 键数 + 1（Go len(workersObj.Keys())+1）。
    let worker_id = workers_count(vm, self_val) as u64 + 1;

    // IPC 监听器须在 spawn 之前绑定：端口 + 一次性握手 key 经环境变量注入子进程
    // （Node 的 ipc 管道在 stdio 数组里传递；本运行时以回环 TCP 承载同一语义）。
    let ipc = cluster_ipc::spawn_worker_listener(worker_id);

    // 当前脚本路径：优先 VM 登记的入口文件（字节码模式下为 .bc 路径，
    // 如 `aluvm run app.bc`——argv[1] 会是子命令 "run"）；回退 argv[1]
    // （源码模式 `aluka app.js`）。
    let script = {
        let entry = vm.entry_file.clone();
        if entry.is_empty() {
            std::env::args()
                .nth(1)
                .filter(|a| !matches!(a.as_str(), "run" | "test" | "-v" | "--version"))
                .unwrap_or_default()
        } else {
            entry
        }
    };

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
        silent: Some(false),
        cwd: String::new(),
        windows_hide: cfg!(windows),
        env: Some(env_pairs),
    };

    let child_val = fork_spawn(vm, script, Vec::new(), opts)?;
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

    // 生命周期状态：实测 Node 22 `cluster.fork()` 返回时 worker 已可视为 online
    // （`isConnected() === true`），故此处直接置 Online。
    set_phase(worker.0, WorkerPhase::Online);
    // child 'exit' → Worker/cluster 'exit'（携带真实退出码）；IPC 帧由事件源泵派发。
    attach_child_wrapper(vm, child_ref.0, "exit", "cluster.__workerExit")?;
    vm.activate_event_source("cluster_ipc", cluster_ipc_pump);

    // workers[id] = worker；同步触发 cluster 'fork'。
    let workers_val = vm.get_property(self_val, "workers")?;
    let _ = vm.set_property(workers_val, &worker_id.to_string(), Value::Object(worker));
    ns_emit(vm, self_val, "fork", &[Value::Object(worker)])?;
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

/// child 'exit' 转接：清理 workers 表 → 置 Dead → cluster `'exit'(worker, code, null)`
/// → worker `'exit'(code, null)`。退出码取自 child `'exit'` 事件实参（真实子进程
/// 状态，不再是 Go 包装的硬编码 0）。
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
    let workers_val = vm.get_property(module_val, "workers")?;
    if let Value::Object(_) = workers_val {
        vm.delete_property(workers_val, &worker_id.to_string());
    }
    // child 'exit'(code, signal)：code 缺失（信号终止等）时按 Node 的 null 语义
    // 传 null；signal 本运行时无法区分，恒 null。
    let code = args.first().copied().unwrap_or(Value::Null);
    set_phase(worker_ref, WorkerPhase::Dead);
    // IPC 通道随进程退出关闭：停掉 accept/读线程并令后续 send 返回 false。
    cluster_ipc::close_channel(worker_id);
    let worker_val = Value::Object(ObjectRef(worker_ref));
    ns_emit(vm, module_val, "exit", &[worker_val, code, Value::Null])?;
    ns_emit(vm, worker_val, "exit", &[code, Value::Null])?;
    Ok(Value::Undefined)
}

/// workers 对象的键数（worker id 个数）。
fn workers_count(vm: &mut Vm, module_val: Value) -> usize {
    let Ok(workers_val) = vm.get_property(module_val, "workers") else {
        return 0;
    };
    vm.own_properties(workers_val).len()
}

/// `cluster.setupMaster([settings])` / `setupPrimary`：写入 settings 的既定键。
fn cluster_setup_master(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let Some(opts) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    if let Value::Object(_) = opts {
        let settings = vm.get_property(self_val, "settings")?;
        for k in [
            "exec",
            "execArgv",
            "args",
            "silent",
            "cwd",
            "serialization",
            "stdio",
        ] {
            if let Ok(v) = vm.get_property(opts, k) {
                if !matches!(v, Value::Undefined) {
                    let _ = vm.set_property(settings, k, v);
                }
            }
        }
    }
    Ok(Value::Undefined)
}

/// `cluster.disconnect([callback])`：逐 worker destroy 后调用回调。
fn cluster_disconnect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let workers_val = vm.get_property(self_val, "workers")?;
    let worker_vals: Vec<Value> = vm
        .own_properties(workers_val)
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    for w in worker_vals {
        if let Ok(destroy_fn) = vm.get_property(w, "destroy") {
            vm.invoke_callable(destroy_fn, w, &[])?;
        }
    }
    if let Some(cb) = args.first().copied() {
        if is_callable(vm, cb) {
            vm.invoke_callable(cb, Value::Undefined, &[])?;
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

/// worker 进程启动时建立 IPC 通道并上报 `'online'`；无通道环境变量/连接失败
/// 返回 false（Node 中 `process.send` 不存在，与 `silent`/无 ipc 的子进程一致）。
///
/// Node 在 bootstrap 阶段建立通道，与是否 `require('cluster')` 无关——故调用点
/// 在解释器构建 process 对象处（见 `interpreter.rs`）。
pub(crate) fn worker_setup_channel() -> bool {
    if !cluster_ipc::child_connect() {
        return false;
    }
    let _ = cluster_ipc::child_send_line(&format!("{FRAME_ONLINE}\n"));
    true
}

/// worker 进程的 IPC 通道是否连通（`process.connected` 初值）。
pub(crate) fn worker_channel_connected() -> bool {
    cluster_ipc::child_is_connected()
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

/// IPC 事件源泵：排空 inbox 中的帧并派发事件；无存活 worker 且队列为空时注销
/// 事件源（否则事件循环会被闲置通道一直挂住）。
fn cluster_ipc_pump(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    while let Some((key, item)) = cluster_ipc::take_incoming() {
        progressed = true;
        match item {
            // key 0 = 子进程自身通道（worker 侧）：worker 不激活本事件源（见模块
            // 文档缺口说明），此处仅丢弃。
            cluster_ipc::Incoming::Line(_) if key == 0 => {}
            cluster_ipc::Incoming::Line(text) => dispatch_worker_frame(vm, key, &text)?,
            // 通道关闭 = worker 断开（Node 'disconnected'：进程未退出仍非 dead）。
            cluster_ipc::Incoming::Closed => mark_disconnected(key),
        }
    }
    if !cluster_ipc_busy() {
        vm.deactivate_event_source("cluster_ipc");
    }
    Ok(progressed)
}

/// 事件源活性：还有非 Dead 的 worker，或 inbox 尚有未派发条目。
fn cluster_ipc_busy() -> bool {
    let alive = WORKER_PHASE.with(|g| {
        g.borrow()
            .as_ref()
            .is_some_and(|m| m.values().any(|p| !matches!(p, WorkerPhase::Dead)))
    });
    alive || cluster_ipc::inbox_pending()
}

/// 通道关闭：worker 仍未退出时状态回落到 `Disconnected`（已 Dead 的不动）。
fn mark_disconnected(worker_id: u64) {
    if let Some(worker_ref) = worker_ref_by_id(worker_id) {
        with_phase_map(|m| {
            if let Some(p) = m.get_mut(&worker_ref) {
                if !matches!(p, WorkerPhase::Dead) {
                    *p = WorkerPhase::Disconnected;
                }
            }
        });
    }
}

/// workers 表中某 id 对应的 worker 对象句柄 id（无此 worker 返回 None）。
fn worker_ref_by_id(worker_id: u64) -> Option<u32> {
    WORKER_BY_ID.with(|g| g.borrow().as_ref().and_then(|m| m.get(&worker_id).copied()))
}

/// 派发一条 worker 帧。
///
/// 帧类型（见 `cluster_ipc` 模块文档）：`o` = online、`m` = message；
/// `l`（listening）/`e`（exitedAfterDisconnect ack）本模块未实现，丢弃。
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
            set_phase(worker_ref, WorkerPhase::Online);
            ns_emit(vm, worker_val, "online", &[])?;
            ns_emit(vm, Value::Object(module_ref), "online", &[worker_val])?;
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

/// 登记 worker id → 对象句柄。
fn register_worker_id(worker_id: u64, worker_ref: u32) {
    WORKER_BY_ID.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(worker_id, worker_ref);
    });
}
