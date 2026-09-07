//! `worker_threads` 内置模块（Phase 6）。
//!
//! 照实移植 Node.js 22 LTS 规范的模型并
//! 按宿主现实落地：Go 侧 worker 是「独立 goroutine + 完整 VM」，Rust 侧 VM
//! 基于 `Rc` 不可跨线程，因此采用**同进程伪 worker（宏任务/事件泵派发）**——
//! 可观测语义与 Go 一致：
//! - `new Worker(path[, opts])`：构造主线程侧 Worker 实例（事件器：
//!   `on/emit/postMessage/terminate/threadId`），worker 模块体经 `proc` 事件
//!   泵加载执行（require 缓存旁路，模块可重复运行）；
//! - worker 内注入 `isMainThread=false`、`parentPort`（消息端口）与
//!   `workerData`（JSON 往返、对象键递归排序——对齐 Go `json.Marshal`），
//!   执行完毕恢复主线程表面并派发 worker `'exit'`（先 `'message'` 后
//!   `'exit'`，Node/Go 语义）；
//! - 消息序列化语义：`postMessage` 值经 JSON 往返克隆（原始值不变、对象键
//!   排序、undefined → null）；
//! - `MessageChannel`/`MessagePort`/`BroadcastChannel`：同进程链接端口 +
//!   消息缓冲（有监听器时异步派发，无监听器时可 `receiveMessageOnPort` 同步取）；
//! - 已知偏离：`{eval: true}` 在字节码 VM 上不可执行（走 worker `'error'` +
//!   `'exit'(1)`）；模块级 `threadId` 恒为 0（Go 同款怪癖）；`SHARE_ENV` 为
//!   普通对象（VM 暂无 Symbol 堆对象）。

use crate::builtins::child_process::proc_common::{
    EMITTER_METHODS, ProcEvent, ns_attach, ns_emit, ns_listener_count, push_event,
    register_ns_emitter_handlers,
};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// `require("worker_threads")` / `require("node:worker_threads")` 模块导出。
pub const MODULE: ModuleDef = ModuleDef {
    name: "worker_threads",
    build,
};

/// worker 实例的线程 id 计数器（Go workerThreadCounter，自 1 起）。
static WORKER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 端口消息缓冲状态（Go msgPortState）。
#[derive(Debug, Default)]
struct PortState {
    /// 待派发消息队列
    queue: VecDeque<Value>,
    /// close 后丢弃新消息
    closed: bool,
}

/// 端口对象句柄 id → 消息缓冲。
static PORT_STATES: Mutex<Option<HashMap<u32, PortState>>> = Mutex::new(None);

fn with_port_state<F, R>(id: u32, f: F) -> R
where
    F: FnOnce(&mut PortState) -> R,
{
    let mut guard = PORT_STATES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map.entry(id).or_default())
}

/// worker 对象句柄 id → parentPort 端口对象句柄 id。
static WORKER_PP: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// parentPort 端口对象句柄 id → worker 对象句柄 id。
static PP_TO_WORKER: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// threadId → Worker 对象句柄 id（postMessageToThread 用）。
static WORKER_BY_THREAD: Mutex<Option<HashMap<u64, u32>>> = Mutex::new(None);

/// 已 terminate 的 worker（后续消息丢弃，Go 关闭通道语义）。
static WORKER_CLOSED: Mutex<Option<std::collections::HashSet<u32>>> = Mutex::new(None);

/// BroadcastChannel：port id → 频道名（广播注册表）。
static BROADCAST_NAMES: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// 频道名 → 成员 port id 列表。
static BROADCAST_CHANNELS: Mutex<Option<HashMap<String, Vec<u32>>>> = Mutex::new(None);

/// 跨 worker 环境数据（Go envDataMap）。
static ENV_DATA: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);

fn with_map<F, R>(holder: &Mutex<Option<HashMap<u32, u32>>>, f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, u32>) -> R,
{
    let mut guard = holder.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new))
}

fn with_thread_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u64, u32>) -> R,
{
    let mut guard = WORKER_BY_THREAD.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new))
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    // 模块级函数导出。
    for method in [
        "markAsUncloneable",
        "markAsUntransferable",
        "isMarkedAsUntransferable",
        "setEnvironmentData",
        "getEnvironmentData",
        "receiveMessageOnPort",
        "postMessageToThread",
        "moveMessagePortToContext",
        "Worker",
        "MessageChannel",
        "MessagePort",
        "BroadcastChannel",
    ] {
        let f = vm.alloc_native_fn(&format!("worker_threads.{method}"));
        set_module_prop(vm, obj, method, Value::Object(f))?;
    }
    register_handler(
        registry,
        "worker_threads",
        "markAsUncloneable",
        wt_mark_noop,
    );
    register_handler(
        registry,
        "worker_threads",
        "markAsUntransferable",
        wt_mark_noop,
    );
    register_handler(
        registry,
        "worker_threads",
        "isMarkedAsUntransferable",
        wt_is_marked,
    );
    register_handler(
        registry,
        "worker_threads",
        "setEnvironmentData",
        wt_set_env_data,
    );
    register_handler(
        registry,
        "worker_threads",
        "getEnvironmentData",
        wt_get_env_data,
    );
    register_handler(
        registry,
        "worker_threads",
        "receiveMessageOnPort",
        wt_receive_on_port,
    );
    register_handler(
        registry,
        "worker_threads",
        "postMessageToThread",
        wt_post_to_thread,
    );
    register_handler(
        registry,
        "worker_threads",
        "moveMessagePortToContext",
        wt_move_port,
    );
    register_handler(registry, "worker_threads", "Worker", wt_worker_ctor);
    register_handler(
        registry,
        "worker_threads",
        "MessageChannel",
        wt_channel_ctor,
    );
    register_handler(registry, "worker_threads", "MessagePort", wt_port_ctor);
    register_handler(
        registry,
        "worker_threads",
        "BroadcastChannel",
        wt_broadcast_ctor,
    );
    // 端口 / parentPort / worker 实例方法命名空间。
    register_ns_emitter_handlers(registry, "worker_threads:port");
    register_handler(
        registry,
        "worker_threads:port",
        "postMessage",
        wt_port_post_message,
    );
    register_handler(registry, "worker_threads:port", "close", wt_port_close);
    register_ns_emitter_handlers(registry, "worker_threads:parent_port");
    register_handler(
        registry,
        "worker_threads:parent_port",
        "postMessage",
        wt_pp_post_message,
    );
    register_handler(
        registry,
        "worker_threads:parent_port",
        "close",
        wt_port_close,
    );
    for m in ["ref", "unref", "start", "hasRef"] {
        register_handler(registry, "worker_threads:port", m, wt_port_noop);
    }
    register_ns_emitter_handlers(registry, "worker_threads:worker");
    register_handler(
        registry,
        "worker_threads:worker",
        "postMessage",
        wt_worker_post,
    );
    register_handler(
        registry,
        "worker_threads:worker",
        "terminate",
        wt_worker_terminate,
    );
    vm.activate_event_source(
        "proc",
        crate::builtins::child_process::proc_common::pump_proc,
    );

    // 主线程默认表面（Go：从全局读取 worker 注入值，缺省 true/0/null）。
    let is_main = match vm.globals.get("isMainThread") {
        Some(Value::Boolean(b)) => *b,
        _ => true,
    };
    let _ = vm.set_property(Value::Object(obj), "isMainThread", Value::Boolean(is_main));
    let _ = vm.set_property(Value::Object(obj), "threadId", Value::Number(0.0));
    let _ = vm.set_property(
        Value::Object(obj),
        "isInternalThread",
        Value::Boolean(false),
    );
    let thread_name = vm.alloc_string(String::new());
    let _ = vm.set_property(Value::Object(obj), "threadName", Value::Object(thread_name));
    let resource_limits = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(obj),
        "resourceLimits",
        Value::Object(resource_limits),
    );
    // Go 为 Symbol；VM 暂无 Symbol 堆对象，以普通对象占位（见模块文档偏离说明）。
    let share_env = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "SHARE_ENV", Value::Object(share_env));
    let parent_port = vm.globals.get("parentPort").copied().unwrap_or(Value::Null);
    let _ = vm.set_property(Value::Object(obj), "parentPort", parent_port);
    let worker_data = vm.globals.get("workerData").copied().unwrap_or(Value::Null);
    let _ = vm.set_property(Value::Object(obj), "workerData", worker_data);
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Worker 构造与 worker 模块体执行
// ---------------------------------------------------------------------------

/// `new Worker(filename[, options])`。
fn wt_worker_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let filename = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    // options：workerData（构造期 JSON 往返）与 eval 标记。
    let mut worker_data: Option<Value> = None;
    let mut eval = false;
    if let Some(opts) = args.get(1).copied() {
        if let Value::Object(o) = opts {
            if let Ok(v) = vm.get_property(opts, "workerData") {
                if !matches!(v, Value::Undefined) {
                    worker_data = Some(json_roundtrip(vm, v));
                }
            }
            if let Ok(Value::Boolean(b)) = vm.get_property(Value::Object(o), "eval") {
                eval = b;
            }
        }
    }

    // 主线程侧 Worker 实例（事件器 + postMessage/terminate）。
    let worker = vm.alloc_ordinary();
    ns_attach(
        vm,
        worker,
        "worker_threads:worker",
        &[
            "on",
            "once",
            "off",
            "emit",
            "listenerCount",
            "postMessage",
            "terminate",
        ],
    );
    let thread_id = WORKER_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = vm.set_property(
        Value::Object(worker),
        "threadId",
        Value::Number(thread_id as f64),
    );
    with_thread_map(|m| {
        m.insert(thread_id, worker.0);
    });

    // worker 端 parentPort 端口（构造期预建，主线程 postMessage 先于 worker
    // 模块体执行时消息在端口缓冲，对齐 Go toWorker 通道缓冲）。
    let pp = make_port(vm, "worker_threads:parent_port");
    with_map(&WORKER_PP, |m| {
        m.insert(worker.0, pp.0);
    });
    with_map(&PP_TO_WORKER, |m| {
        m.insert(pp.0, worker.0);
    });

    if eval {
        // 字节码 VM 无法执行 JS 源码：走 Go 的失败路径（'error' + 'exit'(1)）。
        push_event(ProcEvent::WorkerError {
            worker: worker.0,
            message: "worker: eval:true is not supported by aluka_r (bytecode VM)".to_owned(),
        });
        push_event(ProcEvent::WorkerExit {
            worker: worker.0,
            code: 1,
        });
    } else {
        push_event(ProcEvent::RunWorker {
            worker: worker.0,
            path: filename,
            data: worker_data,
            eval: false,
        });
    }
    vm.activate_event_source(
        "proc",
        crate::builtins::child_process::proc_common::pump_proc,
    );
    Ok(Value::Object(worker))
}

/// worker 模块体执行（`proc` 泵派发）：注入 worker 表面 → require（缓存旁路）
/// → 恢复主线程表面 → 派发 'exit'/'error'。
pub(crate) fn run_worker_body(
    vm: &mut Vm,
    worker_id: u32,
    path: String,
    data: Option<Value>,
    eval: bool,
) -> Result<(), VmError> {
    let worker_val = Value::Object(ObjectRef(worker_id));
    if eval {
        // RunWorker 事件只由非 eval 脚本路径入队；eval 失败在构造期已入队。
        return Ok(());
    }
    let pp_id = with_map(&WORKER_PP, |m| m.get(&worker_id).copied());
    let Some(pp_id) = pp_id else {
        return Ok(());
    };
    let pp_val = Value::Object(ObjectRef(pp_id));

    // worker 脚本对应的 .bc 不存在：复刻 Go loader 的失败文案。
    let Some(bc_path) = resolve_worker_bc(vm, &path) else {
        let abs = absolute_js_path(vm, &path);
        let message = format!(
            "worker: module: module: cannot read \"{abs}\": open {abs}: The system cannot find the file specified."
        );
        let msg = vm.alloc_string(message);
        ns_emit(vm, worker_val, "error", &[Value::Object(msg)])?;
        ns_emit(vm, worker_val, "exit", &[Value::Number(1.0)])?;
        return Ok(());
    };

    // 注入 worker 全局（parentPort / isMainThread / workerData）。
    let saved_pp = vm.globals.insert("parentPort".to_owned(), pp_val);
    let saved_main = vm
        .globals
        .insert("isMainThread".to_owned(), Value::Boolean(false));
    let saved_wd = match data {
        Some(d) => Some(vm.globals.insert("workerData".to_owned(), d)),
        None => Some(vm.globals.remove("workerData")),
    };
    // 同步 worker_threads 模块表面（Go worker VM 内新模块读取注入后的全局）。
    let wt_mod = vm.builtin_registry.module("worker_threads");
    let (old_main, old_pp, old_wd) = if let Some(m) = wt_mod {
        let target = Value::Object(m);
        let old_main = vm.get_property(target, "isMainThread").ok();
        let old_pp = vm.get_property(target, "parentPort").ok();
        let old_wd = vm.get_property(target, "workerData").ok();
        let _ = vm.set_property(target, "isMainThread", Value::Boolean(false));
        let _ = vm.set_property(target, "parentPort", pp_val);
        if let Some(d) = data {
            let _ = vm.set_property(target, "workerData", d);
        }
        (old_main, old_pp, old_wd)
    } else {
        (None, None, None)
    };

    // require 缓存旁路：移除既有 exports，保证同一 worker 文件可重复执行。
    vm.module_exports.remove(&bc_path.display().to_string());
    let spec = Value::Object(vm.alloc_string(path.clone()));
    let run_result = vm.call_require(spec);

    // 恢复主线程全局与模块表面。
    restore_global(vm, "parentPort", saved_pp);
    restore_global(vm, "isMainThread", saved_main);
    if let Some(old) = saved_wd {
        restore_global(vm, "workerData", old);
    }
    if let Some(m) = wt_mod {
        let target = Value::Object(m);
        let _ = vm.set_property(
            target,
            "isMainThread",
            old_main.unwrap_or(Value::Boolean(true)),
        );
        let _ = vm.set_property(target, "parentPort", old_pp.unwrap_or(Value::Null));
        let _ = vm.set_property(target, "workerData", old_wd.unwrap_or(Value::Null));
    }

    match run_result {
        Ok(_) => {
            // 模块体执行期间的 parentPort 缓冲消息此时可派发（监听器已注册）。
            port_flush_pending(vm, pp_id);
            push_event(ProcEvent::WorkerExit {
                worker: worker_id,
                code: 0,
            });
        }
        Err(VmError::Thrown(err_val)) => {
            let message = format!("worker: {}", vm.format_value(err_val));
            push_event(ProcEvent::WorkerError {
                worker: worker_id,
                message,
            });
            push_event(ProcEvent::WorkerExit {
                worker: worker_id,
                code: 1,
            });
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

/// 恢复全局变量：原值为 Some 重写，None 移除。
fn restore_global(vm: &mut Vm, key: &str, old: Option<Value>) {
    match old {
        Some(v) => {
            vm.globals.insert(key.to_owned(), v);
        }
        None => {
            vm.globals.remove(key);
        }
    }
}
/// 解析 worker 路径对应的字节码文件（require 语义：`.js` → `.bc`）。
fn resolve_worker_bc(vm: &Vm, path: &str) -> Option<std::path::PathBuf> {
    let base = vm
        .base_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let rel = path.strip_prefix("./").unwrap_or(path);
    let mut p = base.join(rel);
    match p.extension().and_then(|e| e.to_str()) {
        Some("bc") => {}
        _ => {
            p.set_extension("bc");
        }
    }
    p.is_file().then_some(p)
}

/// worker .js 路径的绝对化（Go loader 错误文案中的路径形态）。
fn absolute_js_path(vm: &Vm, path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return path.to_owned();
    }
    let base = vm
        .base_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let joined = base.join(path);
    if joined.is_absolute() {
        joined.to_string_lossy().to_string()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(joined).to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_owned())
    }
}

// ---------------------------------------------------------------------------
// 端口：MessagePort / MessageChannel / BroadcastChannel / parentPort
// ---------------------------------------------------------------------------

/// 构造一个端口对象（事件器 + postMessage/close[/ref/unref/start/hasRef]）。
fn make_port(vm: &mut Vm, ns: &'static str) -> ObjectRef {
    let port = vm.alloc_ordinary();
    let mut methods: Vec<&str> = EMITTER_METHODS.to_vec();
    methods.extend_from_slice(&["postMessage", "close"]);
    if ns == "worker_threads:port" {
        methods.extend_from_slice(&["ref", "unref", "start", "hasRef"]);
    }
    ns_attach(vm, port, ns, &methods);
    with_port_state(port.0, |_| {});
    vm.activate_event_source(
        "proc",
        crate::builtins::child_process::proc_common::pump_proc,
    );
    port
}

/// `new MessageChannel()` → `{ port1, port2 }` 链接端口对。
fn wt_channel_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let p1 = make_port(vm, "worker_threads:port");
    let p2 = make_port(vm, "worker_threads:port");
    let _ = vm.set_property(Value::Object(p1), "_peer", Value::Object(p2));
    let _ = vm.set_property(Value::Object(p2), "_peer", Value::Object(p1));
    let ch = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(ch), "port1", Value::Object(p1));
    let _ = vm.set_property(Value::Object(ch), "port2", Value::Object(p2));
    Ok(Value::Object(ch))
}

/// `new MessagePort()`：无连接端口（`_peer` 为 null，消息直接丢弃）。
fn wt_port_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let p = make_port(vm, "worker_threads:port");
    let _ = vm.set_property(Value::Object(p), "_peer", Value::Null);
    Ok(Value::Object(p))
}

/// `new BroadcastChannel(name)`：端口 + 频道注册表（postMessage 广播、close 退订）。
fn wt_broadcast_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let port = make_port(vm, "worker_threads:port");
    let name_val = vm.alloc_string(name.clone());
    let _ = vm.set_property(Value::Object(port), "name", Value::Object(name_val));
    BROADCAST_NAMES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(port.0, name.clone());
    BROADCAST_CHANNELS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .entry(name)
        .or_default()
        .push(port.0);
    Ok(Value::Object(port))
}

/// worker 端 parentPort `postMessage`：直接派发到主线程 worker 'message'
/// （Go 直接 PostTask 主线程，无缓冲，先于 'exit'）。
fn wt_pp_post_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let worker = with_map(&PP_TO_WORKER, |m| m.get(&r.0).copied());
    if let Some(worker) = worker {
        let msg = json_roundtrip(vm, args.first().copied().unwrap_or(Value::Undefined));
        push_event(ProcEvent::WorkerToMain { worker, msg });
        vm.activate_event_source(
            "proc",
            crate::builtins::child_process::proc_common::pump_proc,
        );
    }
    Ok(Value::Undefined)
}

/// 端口 `postMessage`：广播端口发全频道，链接端口发给对端缓冲。
fn wt_port_post_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let msg = args.first().copied().unwrap_or(Value::Undefined);

    // BroadcastChannel：投递给同频道其他端口。
    let bc_name = BROADCAST_NAMES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(&r.0)
        .cloned();
    if let Some(name) = bc_name {
        let msg = json_roundtrip(vm, msg);
        let peers: Vec<u32> = BROADCAST_CHANNELS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .get(&name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|id| *id != r.0)
            .collect();
        for peer in peers {
            port_post(vm, Value::Object(ObjectRef(peer)), msg)?;
        }
        return Ok(Value::Undefined);
    }

    // 链接端口：发给 _peer（无对端 → 丢弃，Go 语义一致）。
    let peer = vm.get_property(receiver, "_peer")?;
    if let Value::Object(peer_ref) = peer {
        port_post(vm, Value::Object(peer_ref), msg)?;
    }
    Ok(Value::Undefined)
}

/// 把消息缓冲到 `port` 的端口队列；已有 'message' 监听器时**同步**把队列
/// 整体搬入派发事件（Go deliverPortMessage 语义：缓冲立即清空，
/// receiveMessageOnPort 随即取不到）。
pub(crate) fn port_post(vm: &mut Vm, port: Value, msg: Value) -> Result<(), VmError> {
    let Value::Object(r) = port else {
        return Ok(());
    };
    let msg = json_roundtrip(vm, msg);
    let deliver_now = with_port_state(r.0, |st| {
        if st.closed {
            return false;
        }
        st.queue.push_back(msg);
        ns_listener_count(r.0, "message") > 0
    });
    if deliver_now {
        let msgs: Vec<Value> = with_port_state(r.0, |st| st.queue.drain(..).collect());
        if !msgs.is_empty() {
            push_event(ProcEvent::PortDeliver { port: r.0, msgs });
        }
    }
    Ok(())
}

/// 端口缓冲消息派发入口（worker 模块体结束后 parentPort 的残留缓冲）。
pub(crate) fn port_flush_pending(_vm: &mut Vm, port_id: u32) {
    let has = with_port_state(port_id, |st| {
        !st.queue.is_empty() && ns_listener_count(port_id, "message") > 0
    });
    if has {
        let msgs: Vec<Value> = with_port_state(port_id, |st| st.queue.drain(..).collect());
        push_event(ProcEvent::PortDeliver {
            port: port_id,
            msgs,
        });
    }
}

/// 端口 'message' 事件派发（单条）。
pub(crate) fn port_emit(vm: &mut Vm, port: Value, msg: Value) -> Result<(), VmError> {
    ns_emit(vm, port, "message", &[msg])
}

/// 端口 `close`：closed 后丢新消息；广播端口同时退订频道。
fn wt_port_close(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    with_port_state(r.0, |st| {
        st.closed = true;
        st.queue.clear();
    });
    if let Some(name) = BROADCAST_NAMES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .remove(&r.0)
    {
        if let Some(chans) = BROADCAST_CHANNELS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .get_mut(&name)
        {
            chans.retain(|id| *id != r.0);
        }
    }
    Ok(Value::Undefined)
}

/// `port.ref/unref/start/hasRef`：无事件循环语义，占位（Go 同款简化）。
fn wt_port_noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// worker 实例方法
// ---------------------------------------------------------------------------

/// worker `postMessage(data)`：投递到 worker 端 parentPort 缓冲。
fn wt_worker_post(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let pp = with_map(&WORKER_PP, |m| m.get(&r.0).copied());
    if WORKER_CLOSED
        .lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .contains(&r.0)
    {
        return Ok(Value::Undefined);
    }
    if let Some(pp) = pp {
        let msg = args.first().copied().unwrap_or(Value::Undefined);
        push_event(ProcEvent::MainToWorker { pp, msg });
        vm.activate_event_source(
            "proc",
            crate::builtins::child_process::proc_common::pump_proc,
        );
    }
    Ok(Value::Undefined)
}

/// worker `terminate()`：标记关闭（后续消息丢弃；已入队事件照常派发）。
fn wt_worker_terminate(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    if let Value::Object(r) = receiver {
        WORKER_CLOSED
            .lock()
            .unwrap()
            .get_or_insert_with(Default::default)
            .insert(r.0);
    }
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// 模块级函数
// ---------------------------------------------------------------------------

/// `markAsUncloneable` / `markAsUntransferable`：no-op（Go 同款）。
fn wt_mark_noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `isMarkedAsUntransferable`：恒 false（Go 同款）。
fn wt_is_marked(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// `setEnvironmentData(key[, value])`：缺 value 删除。
fn wt_set_env_data(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(key_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let key = env_data_key(vm, key_val);
    let mut guard = ENV_DATA.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    match args.get(1).copied() {
        Some(v) => {
            map.insert(key, v);
        }
        None => {
            map.remove(&key);
        }
    }
    Ok(Value::Undefined)
}

/// `getEnvironmentData(key)`。
fn wt_get_env_data(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(key_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let key = env_data_key(vm, key_val);
    let guard = ENV_DATA.lock().unwrap();
    Ok(guard
        .as_ref()
        .and_then(|m| m.get(&key))
        .copied()
        .unwrap_or(Value::Undefined))
}

/// 环境数据键序列化（Go workerDataKey：数字 `num:%v`、其余 `str:%s`）。
fn env_data_key(vm: &Vm, v: Value) -> String {
    match v {
        Value::Number(n) => format!("num:{n}"),
        other => format!("str:{}", vm.format_value(other)),
    }
}

/// `receiveMessageOnPort(port)`：同步取一条缓冲消息 → `{ message }` 或 undefined。
fn wt_receive_on_port(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(Value::Object(r)) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let msg = with_port_state(r.0, |st| st.queue.pop_front());
    match msg {
        Some(m) => {
            let obj = vm.alloc_ordinary();
            let _ = vm.set_property(Value::Object(obj), "message", m);
            Ok(Value::Object(obj))
        }
        None => Ok(Value::Undefined),
    }
}

/// `postMessageToThread(threadId, value)`：向指定 worker 的 parentPort 投递。
fn wt_post_to_thread(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (Some(thread_val), Some(msg)) = (args.first().copied(), args.get(1).copied()) else {
        return Ok(Value::Undefined);
    };
    let thread_id = match thread_val {
        Value::Number(n) => n as u64,
        _ => 0,
    };
    let worker = with_thread_map(|m| m.get(&thread_id).copied());
    if let Some(worker) = worker {
        let pp = with_map(&WORKER_PP, |m| m.get(&worker).copied());
        if let Some(pp) = pp {
            push_event(ProcEvent::MainToWorker { pp, msg });
            vm.activate_event_source(
                "proc",
                crate::builtins::child_process::proc_common::pump_proc,
            );
        }
    }
    Ok(Value::Undefined)
}

/// `moveMessagePortToContext(port)`：返回原端口（Go 同款近似）。
fn wt_move_port(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(args.first().copied().unwrap_or(Value::Undefined))
}

// ---------------------------------------------------------------------------
// JSON 往返（Go ValueToJSON + json.Marshal + JSONToEngine 的可观测等价）
// ---------------------------------------------------------------------------

/// 值的 JSON 往返克隆：原始值透传（undefined → null，对齐 `JSON.stringify`
/// 的 null 化），数组保序，对象按键名递归排序（对齐 Go `json.Marshal`）。
pub(crate) fn json_roundtrip(vm: &mut Vm, v: Value) -> Value {
    let Value::Object(r) = v else {
        // undefined → null（JSON 序列化语义）；Number/Boolean/Null 透传。
        return match v {
            Value::Undefined => Value::Null,
            other => other,
        };
    };
    // 先快照堆形态再递归（避免借用冲突）。
    let shape = match vm.heap.get(r.0 as usize) {
        Some(HeapObject::Array { elements, .. }) => JsonShape::Arr(elements.clone()),
        Some(HeapObject::Ordinary { .. }) => {
            let mut pairs: Vec<(String, Value)> = vm.own_entries(r.0 as usize);
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            JsonShape::Obj(pairs)
        }
        // 字符串 / BigInt 透传；其余品类（函数等）按 null 化近似。
        Some(HeapObject::String(_)) | Some(HeapObject::BigInt(_)) => JsonShape::Opaque(v),
        _ => JsonShape::Opaque(Value::Null),
    };
    match shape {
        JsonShape::Opaque(x) => x,
        JsonShape::Arr(elements) => {
            let cloned: Vec<Value> = elements.iter().map(|e| json_roundtrip(vm, *e)).collect();
            Value::Object(vm.alloc_array(cloned))
        }
        JsonShape::Obj(pairs) => {
            let obj = vm.alloc_ordinary();
            for (k, val) in pairs {
                let cloned = json_roundtrip(vm, val);
                let _ = vm.set_property(Value::Object(obj), &k, cloned);
            }
            Value::Object(obj)
        }
    }
}

/// JSON 往返的堆形态快照。
enum JsonShape {
    /// 数组元素快照
    Arr(Vec<Value>),
    /// 对象属性快照（键已排序）
    Obj(Vec<(String, Value)>),
    /// 透传值
    Opaque(Value),
}
