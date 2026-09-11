//! `worker_threads` 内置模块（Phase 6）。
//!
//! 照实移植 Node.js 22 LTS 规范的模型并
//! 按宿主现实落地：Rust 侧 VM 基于 `Rc` 不可跨线程，worker 物理线程经
//! `Vm.worker_entry` 装配层钩子（`aluka-runtime`）spawn，各线程独享 Vm；\n
//! - `new Worker(path[, opts])`：构造主线程侧 Worker 实例（事件器：
//!   `on/emit/postMessage/terminate/threadId`）；真实线程路径下 worker
//!   文件在独立线程的 Vm 上解释执行；无装配钩子时回退到 `proc` 事件泵
//!   派发的同进程伪 worker（require 缓存旁路，模块可重复运行）；\n
//! - worker 内注入 `isMainThread=false`、`parentPort`（消息端口）与
//!   `workerData`（**结构化克隆**：worker_clone 自描述格式，类型面/循环引用
//!   保真，见 `crate::worker_clone`），执行完毕恢复主线程表面并派发\n
//!   worker `'exit'`（先 `'message'` 后 `'exit'`，Node 语义）；\n
//! - 消息序列化语义：`postMessage` 值经**结构化克隆**（基本类型/Date/RegExp/\n
//!   Map/Set/ArrayBuffer/TypedArray/DataView/循环引用；函数/Symbol 抛\n
//!   DataCloneError；transfer list 移交 ArrayBuffer 并 detach 源）；\n
//! - `MessageChannel`/`MessagePort`/`BroadcastChannel`：同进程链接端口 +\n
//!   消息缓冲（有监听器时异步派发，无监听器时可 `receiveMessageOnPort` 同步取）；\n
//! - `postMessageToThread(threadId, value[, transferList][, timeout])`（M5.1）：\n
//!   跨线程 `process.on('workerMessage')` 投递通道（**不经 parentPort**——\n
//!   Node 22.23.1 `lib/internal/worker/messaging.js` 口径），返回 Promise；\n
//! - `{eval: true}`：真实线程路径由装配层现场编译执行（M5.1 收口）；\n
//! - 已知偏离：模块级 `threadId` 恒为 0 的 Go 怪癖已随真实线程路径修正；\n
//!   `SHARE_ENV` 为普通对象（VM 暂无 Symbol 堆对象）；filename 的 URL 实例\n
//!   形态未接受（仅字符串）。\n

use crate::builtins::child_process::proc_common::{
    EMITTER_METHODS, ProcEvent, ns_attach, ns_emit, ns_listener_count, push_event,
    register_ns_emitter_handlers,
};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
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

// 以下状态表全部线程局部：真实 worker 线程持有独立 Vm（独立堆），
// 堆句柄仅在本线程有效，跨线程只能经 `worker::` 通道传 JSON 字符串。

// 端口对象句柄 id → 消息缓冲。
thread_local! {
    static PORT_STATES: RefCell<Option<HashMap<u32, PortState>>> = const { RefCell::new(None) };
}

fn with_port_state<F, R>(id: u32, f: F) -> R
where
    F: FnOnce(&mut PortState) -> R,
{
    PORT_STATES.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        f(map.entry(id).or_default())
    })
}

// worker 对象句柄 id → parentPort 端口对象句柄 id。
thread_local! {
    static WORKER_PP: RefCell<Option<HashMap<u32, u32>>> = const { RefCell::new(None) };
}

// parentPort 端口对象句柄 id → worker 对象句柄 id。
thread_local! {
    static PP_TO_WORKER: RefCell<Option<HashMap<u32, u32>>> = const { RefCell::new(None) };
}

// threadId → Worker 对象句柄 id（postMessageToThread 用）。
thread_local! {
    static WORKER_BY_THREAD: RefCell<Option<HashMap<u64, u32>>> = const { RefCell::new(None) };
}

// 已 terminate 的 worker（后续消息丢弃，Go 关闭通道语义）。
thread_local! {
    static WORKER_CLOSED: RefCell<Option<std::collections::HashSet<u32>>> =
        const { RefCell::new(None) };
}

// BroadcastChannel：port id → 频道名（广播注册表）。
thread_local! {
    static BROADCAST_NAMES: RefCell<Option<HashMap<u32, String>>> = const { RefCell::new(None) };
}

// 频道名 → 成员 port id 列表。
thread_local! {
    static BROADCAST_CHANNELS: RefCell<Option<HashMap<String, Vec<u32>>>> =
        const { RefCell::new(None) };
}

// 跨 worker 环境数据（Go envDataMap）。
thread_local! {
    static ENV_DATA: RefCell<Option<HashMap<String, Value>>> = const { RefCell::new(None) };
}

// 真实跨线程 worker 注册表：worker 对象句柄 id → 主线程侧桥。
thread_local! {
    static REAL_WORKERS: RefCell<HashMap<u32, crate::worker::WorkerBridge>> =
        RefCell::new(HashMap::new());
}

// worker 线程侧 parentPort 端口对象句柄 id（真实线程路径）。
thread_local! {
    static REAL_PP_ID: RefCell<Option<u32>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// postMessageToThread：`process.on('workerMessage')` 跨线程投递通道（M5.1）。
//
// Node 22.23.1 `lib/internal/worker/messaging.js` 语义：投递**不经 parentPort**
// ——在目标线程 `process.emit('workerMessage', value, source)`；返回 Promise
// （resolve undefined），目标线程/监听器缺失 → ERR_WORKER_MESSAGING_FAILED，
// 监听器抛错 → ERR_WORKER_MESSAGING_ERRORED，同线程 → ERR_WORKER_MESSAGING_
// SAME_THREAD，超时 → ERR_WORKER_MESSAGING_TIMEOUT。本实现以路由请求 + ack
// 回程近似 Node 的 SharedArrayBuffer 应答（结果码同款：0/1/2）。
// ---------------------------------------------------------------------------

/// `postMessageToThread` 挂起请求（等待 ack 或超时）。
struct PendingRoute {
    /// 成功路径 resolve 函数（PromiseResolver）
    resolver: Value,
    /// 失败路径 reject 函数（PromiseResolver）
    reject: Value,
    /// 超时定时器 id（无 timeout 参数为 None；ack 先到时据此取消）
    timer: Option<u64>,
}

thread_local! {
    /// 主线程侧挂起表：`(origin_tid, request_id)` → 条目。origin=0 为主线程
    /// 自己的请求；origin=N 为 worker N 发起、经主线程中转的请求。
    static ROUTE_PENDING_MAIN: RefCell<HashMap<(u64, u64), PendingRoute>> =
        RefCell::new(HashMap::new());
    /// worker 线程侧挂起表：本线程发起的请求（request_id → 条目）。
    static ROUTE_PENDING_WORKER: RefCell<HashMap<u64, PendingRoute>> =
        RefCell::new(HashMap::new());
}

/// 按结果码构建投递错误（Node `Error` 实例，带 `code` 自有键）。
fn route_error(vm: &mut Vm, code: &str, message: &str) -> Value {
    let err = vm.alloc_error_instance(message);
    let name = vm.alloc_string("Error".to_owned());
    let code_v = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
    let _ = vm.set_property(Value::Object(err), "code", Value::Object(code_v));
    Value::Object(err)
}

/// Node `Received ...` 检查形态（ERR_INVALID_ARG_TYPE 文本，实测 v22.23.1）：
/// number → `type number (42)`；string → `type string ('abc')`（带单引号）；
/// undefined → `undefined`；null → `null`；布尔 → `type boolean (true)`；
/// 其余对象按 format_value 回退（登记偏离：复杂对象形态不做完整 inspect 复刻）。
fn received_inspect(vm: &Vm, v: Value) -> String {
    match v.case() {
        ValueCase::Number(_) => format!("type number ({})", vm.format_value(v)),
        ValueCase::Boolean(_) => format!("type boolean ({})", vm.format_value(v)),
        Value::Undefined => "undefined".to_owned(),
        Value::Null => "null".to_owned(),
        ValueCase::Object(r) => {
            if vm.is_string_value(Value::Object(r)) {
                format!("type string ('{}')", vm.format_value(v))
            } else {
                vm.format_value(v)
            }
        }
    }
}

/// 投递结果码 → reject 动作（0 成功 resolve undefined；1/2 对应 Node 错误）。
fn settle_route(vm: &mut Vm, pending: PendingRoute, result: u8) -> Result<(), VmError> {
    if let Some(timer) = pending.timer {
        vm.active_timers.insert(timer);
    }
    match result {
        crate::worker::ROUTE_DELIVERED => {
            vm.invoke_callable(pending.resolver, Value::Undefined, &[])?;
        }
        crate::worker::ROUTE_NO_LISTENERS => {
            let reason = route_error(
                vm,
                "ERR_WORKER_MESSAGING_FAILED",
                "Cannot find the destination thread or listener",
            );
            vm.invoke_callable(pending.reject, Value::Undefined, &[reason])?;
        }
        _ => {
            let reason = route_error(
                vm,
                "ERR_WORKER_MESSAGING_ERRORED",
                "The destination thread threw an error while processing the message",
            );
            vm.invoke_callable(pending.reject, Value::Undefined, &[reason])?;
        }
    }
    Ok(())
}

/// 挂起表登记 + 超时定时器（`timeout` 参数存在时）。
/// 定时器回调为带 `_origin`/`_request_id` 自有键的原生函数，到期时若请求
/// 仍在表中则以 ERR_WORKER_MESSAGING_TIMEOUT 拒绝。
fn arm_route_timeout(
    vm: &mut Vm,
    origin: u64,
    request_id: u64,
    timeout_ms: u64,
    main_side: bool,
) -> Result<Option<u64>, VmError> {
    let fire = vm.alloc_native_fn("worker_threads:route_timeout.fire");
    vm.set_native_fn_property(fire, "_origin", Value::Number(origin as f64));
    vm.set_native_fn_property(fire, "_request_id", Value::Number(request_id as f64));
    vm.set_native_fn_property(fire, "_main_side", Value::Boolean(main_side));
    let id_val = crate::builtins::timers::schedule_raw(
        vm,
        Value::Object(fire),
        timeout_ms,
        crate::builtins::test::mock::FakeApi::SetTimeout,
    )?;
    Ok(match id_val.case() {
        ValueCase::Number(n) => Some(n as u64),
        _ => None,
    })
}

/// 当前线程 id（主线程 0；worker 线程为分配的物理线程 id）。
fn current_thread_id() -> u64 {
    crate::worker::worker_thread_io()
        .map(|io| io.thread_id)
        .unwrap_or(0)
}

/// 当前线程是否持有真实 worker 桥（区分真实线程路径与进程内伪 worker）。
fn has_real_workers() -> bool {
    REAL_WORKERS.with(|m| !m.borrow().is_empty())
}

/// threadId → 主线程侧发送端（仅主线程视角可用；桥整体不可克隆，
/// 路由只需要 `to_worker` 端）。
fn sender_by_tid(thread_id: u64) -> Option<std::sync::mpsc::Sender<crate::worker::WorkerInbound>> {
    let worker = with_thread_map(|m| m.get(&thread_id).copied())?;
    REAL_WORKERS.with(|m| m.borrow().get(&worker).map(|b| b.to_worker.clone()))
}

/// 在**本线程** process 对象上投递 `workerMessage`（返回 Node 结果码）。
/// 监听器缺失 → 1；监听器抛错 → 吞错返回 2（Node `receiveMessageFromWorker`
/// 的 `catch {}` 口径）；成功 → 0。
fn emit_worker_message_on_current_thread(
    vm: &mut Vm,
    value: Value,
    source: u64,
) -> Result<u8, VmError> {
    let Some(process) = vm.process_object else {
        return Ok(crate::worker::ROUTE_NO_LISTENERS);
    };
    if ns_listener_count(process.0, "workerMessage") == 0 {
        return Ok(crate::worker::ROUTE_NO_LISTENERS);
    }
    match ns_emit(
        vm,
        Value::Object(process),
        "workerMessage",
        &[value, Value::Number(source as f64)],
    ) {
        Ok(()) => Ok(crate::worker::ROUTE_DELIVERED),
        Err(VmError::Thrown(_)) => Ok(crate::worker::ROUTE_LISTENER_ERROR),
        Err(e) => Err(e),
    }
}

/// `postMessageToThread` 超时定时器回调：请求仍在挂起表 → 摘除并以
/// ERR_WORKER_MESSAGING_TIMEOUT 拒绝（ack 已先到的请求不在表中，no-op）。
fn wt_route_timeout_fire(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let ValueCase::Object(r) = callee.case() else {
        return Ok(Value::Undefined);
    };
    let read_num = |key: &str| -> Option<u64> {
        match vm.get_native_fn_property(r, key).map(|v| v.case()) {
            Some(ValueCase::Number(n)) if n >= 0.0 => Some(n as u64),
            _ => None,
        }
    };
    let (Some(request_id), origin, main_side) = (
        read_num("_request_id"),
        read_num("_origin"),
        matches!(
            vm.get_native_fn_property(r, "_main_side"),
            Some(ValueCase::Boolean(true))
        ),
    ) else {
        return Ok(Value::Undefined);
    };
    let pending = if main_side {
        ROUTE_PENDING_MAIN.with(|g| g.borrow_mut().remove(&(origin.unwrap_or(0), request_id)))
    } else {
        ROUTE_PENDING_WORKER.with(|g| g.borrow_mut().remove(&request_id))
    };
    if let Some(pending) = pending {
        let err = route_error(
            vm,
            "ERR_WORKER_MESSAGING_TIMEOUT",
            "Sending a message to another thread timed out",
        );
        vm.invoke_callable(pending.reject, Value::Undefined, &[err])?;
    }
    Ok(Value::Undefined)
}

fn with_worker_pp<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, u32>) -> R,
{
    WORKER_PP.with(|g| {
        let mut binding = g.borrow_mut();
        f(binding.get_or_insert_with(HashMap::new))
    })
}

fn with_pp_to_worker<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, u32>) -> R,
{
    PP_TO_WORKER.with(|g| {
        let mut binding = g.borrow_mut();
        f(binding.get_or_insert_with(HashMap::new))
    })
}

fn with_thread_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u64, u32>) -> R,
{
    WORKER_BY_THREAD.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
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
        "worker_threads:route_timeout",
        "fire",
        wt_route_timeout_fire,
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
    // ref/unref/start/hasRef：端口与 parentPort（MessagePort 子类）同一方法面。
    // Node 22 实测：ref()/unref() 返回 undefined，hasRef() 默认 true、unref() 后 false。
    for ns in ["worker_threads:port", "worker_threads:parent_port"] {
        register_handler(registry, ns, "ref", wt_port_ref);
        register_handler(registry, ns, "unref", wt_port_unref);
        register_handler(registry, ns, "start", wt_port_start);
        register_handler(registry, ns, "hasRef", wt_port_has_ref);
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
    let is_main = match vm.globals.get("isMainThread").map(|v| v.case()) {
        Some(ValueCase::Boolean(b)) => b,
        _ => true,
    };
    let _ = vm.set_property(Value::Object(obj), "isMainThread", Value::Boolean(is_main));
    let tid = crate::worker::worker_thread_io()
        .map(|io| io.thread_id)
        .unwrap_or(0);
    let _ = vm.set_property(Value::Object(obj), "threadId", Value::Number(tid as f64));
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
    // options：workerData（构造期结构化克隆往返）与 eval 标记。
    let mut worker_data: Option<Value> = None;
    let mut eval = false;
    if let Some(opts) = args.get(1).copied() {
        if let Some(o) = opts.as_object() {
            if let Ok(v) = vm.get_property(opts, "workerData") {
                if !matches!(v, Value::Undefined) {
                    worker_data = Some(json_roundtrip(vm, v)?);
                }
            }
            if let Ok(ValueCase::Boolean(b)) = vm.get_property(Value::Object(o), "eval").map(ValueCase::from) {
                eval = b;
            }
        }
    }

    // filename 类型校验（Node 22 实测口径）：
    // - 非字符串 + eval !== true → ERR_INVALID_ARG_TYPE（同步抛）；
    // - 非字符串 + eval === true → ERR_INVALID_ARG_VALUE（同步抛）。
    let raw_first = args.first().copied().unwrap_or(Value::Undefined);
    if !vm.is_string_value(raw_first) {
        // 命中 ARG_VALUE 分支要求 options.eval === true，故 Received 恒为 true
        let (message, code_name) = if eval {
            (
                "The property 'options.eval' must be false when 'filename' is not a string. Received true".to_owned(),
                "ERR_INVALID_ARG_VALUE",
            )
        } else {
            (
                format!(
                    "The \"filename\" argument must be of type string or an instance of URL. Received {}",
                    received_inspect(vm, raw_first)
                ),
                "ERR_INVALID_ARG_TYPE",
            )
        };
        let err = vm.alloc_error_instance(&message);
        let name = vm.alloc_string("TypeError".to_owned());
        let code = vm.alloc_string(code_name.to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        let _ = vm.set_property(Value::Object(err), "code", Value::Object(code));
        return Err(VmError::Thrown(Value::Object(err)));
    }
    let filename = vm.format_value(raw_first);

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
    with_worker_pp(|m| {
        m.insert(worker.0, pp.0);
    });
    with_pp_to_worker(|m| {
        m.insert(pp.0, worker.0);
    });

    // 源码串提取（eval 形态：第一参数即源码）。
    let source_text: Option<String> = match raw_first.case() {
        ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(crate::heap::HeapObject::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };

    if eval && vm.worker_entry.is_none() {
        // 伪 worker（无装配钩子）无法执行 eval 源码：Go 的失败路径。
        push_event(ProcEvent::WorkerError {
            worker: worker.0,
            message: "worker: eval:true is not supported by aluka_r (bytecode VM)".to_owned(),
        });
        push_event(ProcEvent::WorkerExit {
            worker: worker.0,
            code: 1,
        });
    } else if let Some(entry) = vm.worker_entry.clone() {
        // M5.1 真实跨物理线程路径：装配层钩子负责编译并运行 worker
        // （文件路径或 eval 源码），消息只以结构化克隆字节跨线程。
        let source = if eval {
            crate::worker::WorkerSource::Eval(source_text.unwrap_or_default())
        } else {
            let abs = absolute_js_path(vm, &filename);
            crate::worker::WorkerSource::File(abs)
        };
        let data_json = match worker_data {
            Some(d) => Some(value_to_json_string(vm, d)?),
            None => None,
        };
        match entry(source, data_json.as_deref()) {
            Ok(bridge) => {
                let _ = vm.set_property(
                    Value::Object(worker),
                    "threadId",
                    Value::Number(bridge.thread_id as f64),
                );
                let tid = bridge.thread_id;
                with_thread_map(|m| {
                    m.insert(tid, worker.0);
                });
                REAL_WORKERS.with(|m| {
                    m.borrow_mut().insert(worker.0, bridge);
                });
                vm.activate_event_source("real_workers", pump_real_workers);
            }
            Err(message) => {
                push_event(ProcEvent::WorkerError {
                    worker: worker.0,
                    message,
                });
                push_event(ProcEvent::WorkerExit {
                    worker: worker.0,
                    code: 1,
                });
            }
        }
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
    let pp_id = with_worker_pp(|m| m.get(&worker_id).copied());
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
/// 解析优先级：绝对路径 → 相对 cwd 可达（`__filename` 入口相对形态）→
/// 相对入口目录（require 语义）→ 回退 cwd 拼接。
fn absolute_js_path(vm: &Vm, path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return path.to_owned();
    }
    if let Ok(cwd) = std::env::current_dir() {
        let as_cwd = cwd.join(path);
        if as_cwd.is_file() {
            return as_cwd.to_string_lossy().to_string();
        }
    }
    let base = vm
        .base_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let joined = base.join(path);
    if joined.is_file() {
        if joined.is_absolute() {
            return joined.to_string_lossy().to_string();
        }
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(joined).to_string_lossy().to_string();
        }
    }
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
    // ref/unref/start/hasRef 同为 MessagePort 子类（parentPort）的方法面
    if matches!(ns, "worker_threads:port" | "worker_threads:parent_port") {
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

/// `new MessagePort()`：Node 抛 ERR_ILLEGAL_CONSTRUCTOR（端口只能经
/// `MessageChannel`/`worker` 获得；原实现降级为无连接端口已修正）。
fn wt_port_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let err = vm.alloc_error_instance("Illegal constructor");
    let n = vm.alloc_string("TypeError".to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(n));
    let c = vm.alloc_string("ERR_ILLEGAL_CONSTRUCTOR".to_owned());
    let _ = vm.set_property(Value::Object(err), "code", Value::Object(c));
    Err(VmError::Thrown(Value::Object(err)))
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
    BROADCAST_NAMES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(port.0, name.clone());
    });
    BROADCAST_CHANNELS.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .entry(name)
            .or_default()
            .push(port.0);
    });
    Ok(Value::Object(port))
}

/// worker 端 parentPort `postMessage`：直接派发到主线程 worker 'message'
/// （Go 直接 PostTask 主线程，无缓冲，先于 'exit'）。
fn wt_pp_post_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Undefined);
    };
    // M5.1 真实线程路径：worker 侧 parentPort → 主线程通道
    if crate::worker::is_worker_thread() && REAL_PP_ID.with(|c| *c.borrow()) == Some(r.0) {
        if let Some(io) = crate::worker::worker_thread_io() {
            let msg = json_roundtrip(vm, args.first().copied().unwrap_or(Value::Undefined))?;
            let json = value_to_json_string(vm, msg)?;
            let _ = io.to_main.send(crate::worker::WorkerEvent::Message(json));
        }
        return Ok(Value::Undefined);
    }
    let worker = with_pp_to_worker(|m| m.get(&r.0).copied());
    if let Some(worker) = worker {
        let msg = json_roundtrip(vm, args.first().copied().unwrap_or(Value::Undefined))?;
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
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Undefined);
    };
    let msg = args.first().copied().unwrap_or(Value::Undefined);

    // BroadcastChannel：投递给同频道其他端口。
    let bc_name = BROADCAST_NAMES.with(|g| g.borrow().as_ref().and_then(|m| m.get(&r.0)).cloned());
    if let Some(name) = bc_name {
        let msg = json_roundtrip(vm, msg)?;
        let peers: Vec<u32> = BROADCAST_CHANNELS.with(|g| {
            g.borrow()
                .as_ref()
                .and_then(|m| m.get(&name))
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|id| *id != r.0)
                .collect()
        });
        for peer in peers {
            port_post(vm, Value::Object(ObjectRef(peer)), msg)?;
        }
        return Ok(Value::Undefined);
    }

    // 链接端口：发给 _peer（无对端 → 丢弃，Go 语义一致）。
    let peer = vm.get_property(receiver, "_peer")?;
    if let Some(peer_ref) = peer.as_object() {
        port_post(vm, Value::Object(peer_ref), msg)?;
    }
    Ok(Value::Undefined)
}

/// 把消息缓冲到 `port` 的端口队列；已有 'message' 监听器时**同步**把队列
/// 整体搬入派发事件（Go deliverPortMessage 语义：缓冲立即清空，
/// receiveMessageOnPort 随即取不到）。
pub(crate) fn port_post(vm: &mut Vm, port: Value, msg: Value) -> Result<(), VmError> {
    let ValueCase::Object(r) = port.case() else {
        return Ok(());
    };
    let msg = json_roundtrip(vm, msg)?;
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
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Undefined);
    };
    with_port_state(r.0, |st| {
        st.closed = true;
        st.queue.clear();
    });
    let removed =
        BROADCAST_NAMES.with(|g| g.borrow_mut().get_or_insert_with(HashMap::new).remove(&r.0));
    if let Some(name) = removed {
        BROADCAST_CHANNELS.with(|g| {
            if let Some(chans) = g
                .borrow_mut()
                .get_or_insert_with(HashMap::new)
                .get_mut(&name)
            {
                chans.retain(|id| *id != r.0);
            }
        });
    }
    Ok(Value::Undefined)
}

thread_local! {
    /// 端口引用状态（`MessagePort.ref()` / `unref()` / `hasRef()` 的可见面）。
    ///
    /// Node 22.23.1 实测口径：`ref()`/`unref()` **返回 undefined**（不是 this），
    /// `hasRef()` 默认 `true`、`unref()` 之后为 `false`。本运行时没有真正的事件
    /// 循环引用计数（端口存活由 proc 事件源泵决定），故只维护 `hasRef()` 的可见值；
    /// 缺省（表内无记录）= ref'd，与 Node 一致。
    static PORT_HAS_REF: RefCell<HashMap<u32, bool>> = RefCell::new(HashMap::new());
}

/// 写入当前接收者（端口对象）的 ref 状态。
fn port_set_has_ref(has: bool) {
    if let Some(r) = crate::builtins::current_receiver().as_object() {
        PORT_HAS_REF.with(|m| {
            m.borrow_mut().insert(r.0, has);
        });
    }
}

/// `port.ref()`：标记为 ref'd（返回 undefined——Node 22 实测口径）。
fn wt_port_ref(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    port_set_has_ref(true);
    Ok(Value::Undefined)
}

/// `port.unref()`：标记为 unref'd（返回 undefined——Node 22 实测口径）。
fn wt_port_unref(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    port_set_has_ref(false);
    Ok(Value::Undefined)
}

/// `port.start()`：开始接收消息（本运行时端口随 `on('message')` 即生效）；返回 undefined。
fn wt_port_start(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `port.hasRef()`：是否处于 ref 状态（默认 true，`unref()` 后 false）。
fn wt_port_has_ref(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let has = match crate::builtins::current_receiver().case() {
        ValueCase::Object(r) => PORT_HAS_REF.with(|m| m.borrow().get(&r.0).copied().unwrap_or(true)),
        _ => true,
    };
    Ok(Value::Boolean(has))
}

// ---------------------------------------------------------------------------
// worker 实例方法
// ---------------------------------------------------------------------------

/// worker `postMessage(data)`：投递到 worker 端 parentPort 缓冲。
fn wt_worker_post(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Undefined);
    };
    let pp = with_worker_pp(|m| m.get(&r.0).copied());
    if WORKER_CLOSED.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(Default::default)
            .contains(&r.0)
    }) {
        return Ok(Value::Undefined);
    }
    // M5.1 真实线程路径：结构化克隆 + transfer（经 base64 通道到物理线程）
    if REAL_WORKERS.with(|m| m.borrow().contains_key(&r.0)) {
        if let Some(bridge) =
            REAL_WORKERS.with(|m| m.borrow().get(&r.0).map(|b| b.to_worker.clone()))
        {
            let msg = args.first().copied().unwrap_or(Value::Undefined);
            // transfer list（第二参数数组元素；解析失败按不可转移报错）
            let transfer = collect_transfer_list(vm, args.get(1).copied())?;
            let bytes = crate::worker_clone::serialize(vm, msg, &transfer)?;
            let json = b64_encode(&bytes);
            let _ = bridge.send(crate::worker::WorkerInbound::PortMessage(json));
        }
        return Ok(Value::Undefined);
    }
    if let Some(pp) = pp {
        let msg = args.first().copied().unwrap_or(Value::Undefined);
        let msg = json_roundtrip(vm, msg)?;
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
    if let Some(r) = receiver.as_object() {
        WORKER_CLOSED.with(|g| {
            g.borrow_mut()
                .get_or_insert_with(Default::default)
                .insert(r.0)
        });
        // 真实线程路径：置终止旗标、关闭通道、立即派发 'exit'(1)（Node 语义）
        if let Some(bridge) = REAL_WORKERS.with(|m| m.borrow_mut().remove(&r.0)) {
            bridge
                .terminate
                .store(true, std::sync::atomic::Ordering::SeqCst);
            ns_emit(_vm, Value::Object(r), "exit", &[Value::Number(1.0)])?;
            maybe_deactivate_real_worker_source(_vm);
        }
    }
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// 模块级函数
// ---------------------------------------------------------------------------

/// `markAsUncloneable`：no-op（Go 同款）；`markAsUntransferable(buf)`：
/// 登记缓冲句柄——此后出现在 transfer list 时抛 DataCloneError（Node 实测
/// `Cannot transfer object of unsupported type.`，与二次 transfer 同文本）。
fn wt_mark_noop(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(r) = args.first().and_then(|v| v.as_object()) {
        crate::worker_clone::mark_untransferable(r);
    }
    let _ = vm;
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
    ENV_DATA.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        match args.get(1).copied() {
            Some(v) => {
                map.insert(key, v);
            }
            None => {
                map.remove(&key);
            }
        }
    });
    Ok(Value::Undefined)
}

/// `getEnvironmentData(key)`。
fn wt_get_env_data(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(key_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let key = env_data_key(vm, key_val);
    Ok(ENV_DATA.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&key))
            .copied()
            .unwrap_or(Value::Undefined)
    }))
}

/// 环境数据键序列化（Go workerDataKey：数字 `num:%v`、其余 `str:%s`）。
fn env_data_key(vm: &Vm, v: Value) -> String {
    match v.case() {
        ValueCase::Number(n) => format!("num:{n}"),
        other => format!("str:{}", vm.format_value(other)),
    }
}

/// `receiveMessageOnPort(port)`：同步取一条缓冲消息 → `{ message }` 或 undefined。
fn wt_receive_on_port(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(r) = args.first().copied().and_then(|v| v.as_object()) else {
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

/// `postMessageToThread(threadId, value[, transferList][, timeout])`：
/// 向指定线程的 `process.on('workerMessage')` 监听器投递（Node 22.23.1 口径，
/// 返回 Promise；transferList 为数字时按 `timeout` 重载解释）。
fn wt_post_to_thread(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 参数重载：`transferList` 为数字且 `timeout` 未传 → 数字即 timeout
    let mut transfer_arg = args.get(2).copied();
    let mut timeout_arg = args.get(3).copied();
    if let Some(_) = transfer_arg.and_then(|v| v.as_number()) {
        if timeout_arg.is_none() {
            timeout_arg = transfer_arg.take();
        }
    }

    // Promise 面（校验失败也走 rejection——Node async 函数语义）
    let promise = vm.alloc_pending_promise();
    let resolve = Value::Object(vm.alloc_promise_resolver(promise, true));
    let reject = Value::Object(vm.alloc_promise_resolver(promise, false));

    // timeout 校验：validateNumber(timeout, 'timeout', 0)
    if let Some(t) = timeout_arg {
        let ok = match t.case() {
            ValueCase::Number(n) if n >= 0.0 => true,
            ValueCase::Number(_) => false,
            _ => false,
        };
        if !ok {
            let reason = match t.case() {
                ValueCase::Number(n) => {
                    let err = vm.alloc_error_instance(&format!(
                        "The value of \"timeout\" is out of range. It must be >= 0. Received {n}"
                    ));
                    let name = vm.alloc_string("RangeError".to_owned());
                    let code = vm.alloc_string("ERR_OUT_OF_RANGE".to_owned());
                    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                    let _ = vm.set_property(Value::Object(err), "code", Value::Object(code));
                    Value::Object(err)
                }
                other => {
                    let err = vm.alloc_error_instance(&format!(
                        "The \"timeout\" argument must be of type number. Received {}",
                        received_inspect(vm, other)
                    ));
                    let name = vm.alloc_string("TypeError".to_owned());
                    let code = vm.alloc_string("ERR_INVALID_ARG_TYPE".to_owned());
                    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                    let _ = vm.set_property(Value::Object(err), "code", Value::Object(code));
                    Value::Object(err)
                }
            };
            vm.invoke_callable(reject, Value::Undefined, &[reason])?;
            return Ok(Value::Object(promise));
        }
    }

    let current_tid = current_thread_id();
    // 同线程判定：仅数值与当前线程 id 相等才命中（Node `===` 语义）
    let same_thread = matches!(args.first(), Some(ValueCase::Number(n)) if *n as u64 == current_tid && n.fract() == 0.0);
    if same_thread {
        let err = route_error(
            vm,
            "ERR_WORKER_MESSAGING_SAME_THREAD",
            "Cannot sent a message to the same thread",
        );
        vm.invoke_callable(reject, Value::Undefined, &[err])?;
        return Ok(Value::Object(promise));
    }

    let destination = match args.first().copied().map(|v| v.case()) {
        Some(ValueCase::Number(n)) if n >= 0.0 => n as u64,
        _ => u64::MAX, // 非数值/负数：无匹配线程 → FAILED 路径
    };
    let msg = args.get(1).copied().unwrap_or(Value::Undefined);

    // 主线程 + 存在真实 worker 桥：直接经桥投递（destination 0 在主线程
    // 上发起即同线程，已在上方拒绝）。
    if current_tid == 0 && has_real_workers() {
        if let Some(bridge) = sender_by_tid(destination) {
            let transfer = collect_transfer_list(vm, transfer_arg)?;
            let bytes = crate::worker_clone::serialize(vm, msg, &transfer)?;
            let json = b64_encode(&bytes);
            let request_id = crate::worker::next_route_request_id();
            let timer = match timeout_arg.map(|v| v.case()) {
                Some(ValueCase::Number(n)) => {
                    arm_route_timeout(vm, 0, request_id, n.max(0.0) as u64, true)?
                }
                _ => None,
            };
            ROUTE_PENDING_MAIN.with(|g| {
                g.borrow_mut().insert(
                    (0, request_id),
                    PendingRoute {
                        resolver: resolve,
                        reject,
                        timer,
                    },
                );
            });
            let _ = bridge.send(crate::worker::WorkerInbound::WorkerMessage {
                request_id,
                source: 0,
                json,
            });
            vm.activate_event_source("real_workers", pump_real_workers);
            return Ok(Value::Object(promise));
        }
        // 目标线程不存在（或为伪 worker）：Node threadsPorts.get 缺失 → FAILED
        let err = route_error(
            vm,
            "ERR_WORKER_MESSAGING_FAILED",
            "Cannot find the destination thread or listener",
        );
        vm.invoke_callable(reject, Value::Undefined, &[err])?;
        return Ok(Value::Object(promise));
    }

    // worker 线程发起：一律经主线程中转/投递（RouteRequest）
    if let Some(io) = crate::worker::worker_thread_io() {
        let transfer = collect_transfer_list(vm, transfer_arg)?;
        let bytes = crate::worker_clone::serialize(vm, msg, &transfer)?;
        let json = b64_encode(&bytes);
        let request_id = crate::worker::next_route_request_id();
        let timer = match timeout_arg.map(|v| v.case()) {
            Some(ValueCase::Number(n)) => {
                arm_route_timeout(vm, io.thread_id, request_id, n.max(0.0) as u64, false)?
            }
            _ => None,
        };
        ROUTE_PENDING_WORKER.with(|g| {
            g.borrow_mut().insert(
                request_id,
                PendingRoute {
                    resolver: resolve,
                    reject,
                    timer,
                },
            );
        });
        let _ = io.to_main.send(crate::worker::WorkerEvent::RouteRequest {
            request_id,
            source: io.thread_id,
            destination,
            json,
        });
        return Ok(Value::Object(promise));
    }

    // 伪 worker 路径（无装配钩子/非真实线程）：同 VM 内直接投递——
    // process 'workerMessage' 监听器为共享表面，投递结果同步可得。
    if destination == u64::MAX {
        let err = route_error(
            vm,
            "ERR_WORKER_MESSAGING_FAILED",
            "Cannot find the destination thread or listener",
        );
        vm.invoke_callable(reject, Value::Undefined, &[err])?;
        return Ok(Value::Object(promise));
    }
    let target_known = with_thread_map(|m| m.contains_key(&destination));
    if !target_known {
        let err = route_error(
            vm,
            "ERR_WORKER_MESSAGING_FAILED",
            "Cannot find the destination thread or listener",
        );
        vm.invoke_callable(reject, Value::Undefined, &[err])?;
        return Ok(Value::Object(promise));
    }
    let cloned = json_roundtrip(vm, msg)?;
    let result = emit_worker_message_on_current_thread(vm, cloned, current_tid)?;
    let reason = route_error(
        vm,
        "ERR_WORKER_MESSAGING_FAILED",
        "Cannot find the destination thread or listener",
    );
    match result {
        crate::worker::ROUTE_DELIVERED => {
            vm.invoke_callable(resolve, Value::Undefined, &[])?;
        }
        _ => {
            vm.invoke_callable(reject, Value::Undefined, &[reason])?;
        }
    }
    Ok(Value::Object(promise))
}

/// `moveMessagePortToContext(port)`：返回原端口（Go 同款近似）。
fn wt_move_port(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(args.first().copied().unwrap_or(Value::Undefined))
}

// ---------------------------------------------------------------------------
// 结构化克隆（M5.1）：跨线程传值 / 端口消息 / workerData 统一走
// `worker_clone` 自描述字节格式（原 JSON 往返：键排序、undefined→null、
// 函数与 ArrayBuffer null 化——已废弃；保留函数名减少通路改动面）
// ---------------------------------------------------------------------------

/// 值的结构化克隆往返（同线程深克隆：端口队列与进程内派发用；
/// 不可克隆值抛 DataCloneError——对齐 Node `postMessage`）。
pub(crate) fn json_roundtrip(vm: &mut Vm, v: Value) -> Result<Value, VmError> {
    let bytes = crate::worker_clone::serialize(vm, v, &[])?;
    crate::worker_clone::deserialize(vm, &bytes)
        .map_err(|_| crate::worker_clone::data_clone_error(vm, "Object could not be cloned."))
}

// ---------------------------------------------------------------------------
// M5.1 真实跨物理线程：主线程泵 + worker 线程事件循环
// ---------------------------------------------------------------------------

/// 值 → 传输字符串（跨线程通道载荷 = 结构化克隆字节的 base64）。
fn value_to_json_string(vm: &mut Vm, v: Value) -> Result<String, VmError> {
    let bytes = crate::worker_clone::serialize(vm, v, &[])?;
    Ok(b64_encode(&bytes))
}

/// 传输字符串 → 值（接收线程堆上重建；坏载荷按丢弃语义返回 Err）。
fn json_string_to_value(vm: &mut Vm, payload: &str) -> Result<Value, VmError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| crate::worker_clone::data_clone_error(vm, "could not be cloned."))?;
    crate::worker_clone::deserialize(vm, &bytes)
        .map_err(|_| crate::worker_clone::data_clone_error(vm, "could not be cloned."))
}

fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// transfer list 收集（`postMessage(value, transferList)` 第二参数）：
/// undefined/null/缺省 → 空；数组 → 元素列表（元素合法性由序列化端校验）。
fn collect_transfer_list(vm: &mut Vm, v: Option<Value>) -> Result<Vec<Value>, VmError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    if matches!(v, Value::Undefined | Value::Null) {
        return Ok(Vec::new());
    }
    match v.case() {
        ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::Array { elements, .. }) => Ok(elements.clone()),
            _ => Ok(Vec::new()),
        },
        _ => Ok(Vec::new()),
    }
}

/// 主线程侧真实 worker 泵：非阻塞收取各 worker 线程事件并派发
/// `'message'` / `'error'` / `'exit'`，处理 `postMessageToThread` 路由请求
/// 与 ack 回程（`real_workers` 事件源）。
fn pump_real_workers(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    // 先全量取走事件（borrow 不跨 VM 调用）
    let events: Vec<(u32, crate::worker::WorkerEvent)> = REAL_WORKERS.with(|m| {
        let map = m.borrow();
        let mut out = Vec::new();
        for (wid, bridge) in map.iter() {
            while let Ok(ev) = bridge.from_worker.try_recv() {
                out.push((*wid, ev));
            }
        }
        out
    });
    let mut exits: Vec<(u32, u32)> = Vec::new();
    for (wid, ev) in events {
        progressed = true;
        let target = Value::Object(ObjectRef(wid));
        match ev {
            crate::worker::WorkerEvent::Message(json) => {
                if let Ok(val) = json_string_to_value(vm, &json) {
                    ns_emit(vm, target, "message", &[val])?;
                }
            }
            crate::worker::WorkerEvent::Error { name, message } => {
                // 主线程 'error' 收到 Error 对象（name/message 保真，对齐 Node）
                let err = vm.alloc_error_instance(&message);
                let name_v = vm.alloc_string(name);
                let _ = vm.set_property(Value::Object(err), "name", Value::Object(name_v));
                ns_emit(vm, target, "error", &[Value::Object(err)])?;
            }
            crate::worker::WorkerEvent::RouteRequest {
                request_id,
                source,
                destination,
                json,
            } => {
                // worker → 主线程（destination 0）直接投递；其余经
                // threadsPorts 表中转到目标 worker（Node 主线程角色）。
                if destination == 0 {
                    let result = match json_string_to_value(vm, &json) {
                        Ok(val) => emit_worker_message_on_current_thread(vm, val, source)?,
                        Err(_) => crate::worker::ROUTE_LISTENER_ERROR,
                    };
                    if let Some(sender) = sender_by_tid(source) {
                        let _ = sender
                            .send(crate::worker::WorkerInbound::RouteAck { request_id, result });
                    }
                } else if let Some(sender) = sender_by_tid(destination) {
                    let _ = sender.send(crate::worker::WorkerInbound::WorkerMessage {
                        request_id,
                        source,
                        json,
                    });
                } else if let Some(sender) = sender_by_tid(source) {
                    let _ = sender.send(crate::worker::WorkerInbound::RouteAck {
                        request_id,
                        result: crate::worker::ROUTE_NO_LISTENERS,
                    });
                }
            }
            crate::worker::WorkerEvent::RouteAck {
                origin,
                request_id,
                result,
            } => {
                // 目标 worker 的投递结果：origin=0 → 主线程自己的请求结算；
                // origin=N → 转发给发起 worker N（其挂起表按 request_id 结算）。
                let pending =
                    ROUTE_PENDING_MAIN.with(|g| g.borrow_mut().remove(&(origin, request_id)));
                match pending {
                    Some(p) => settle_route(vm, p, result)?,
                    None => {
                        if origin != 0 {
                            if let Some(sender) = sender_by_tid(origin) {
                                let _ = sender.send(crate::worker::WorkerInbound::RouteAck {
                                    request_id,
                                    result,
                                });
                            }
                        }
                    }
                }
            }
            crate::worker::WorkerEvent::Exit(code) => {
                // M5.1 修复（双发竞态）：Exit 事件必须复查存活——同批
                // [Message, Exit] 序列中，Message 回调里 terminate() 已同步
                // emit 'exit'(1) 并 remove 桥；滞留的 Exit(0) 若照常派发会
                // 双发 'exit'。已 terminate（不在 REAL_WORKERS）→ 丢弃。
                let alive = REAL_WORKERS.with(|m| m.borrow().contains_key(&wid));
                if !alive {
                    continue;
                }
                exits.push((wid, code));
            }
        }
    }
    for (wid, code) in exits {
        // 二次防护：收集期间回调可能已 terminate 该 worker（exit(1) 已
        // 派发、桥已 remove）——滞留 Exit 丢弃，避免双发。
        if !REAL_WORKERS.with(|m| m.borrow().contains_key(&wid)) {
            continue;
        }
        REAL_WORKERS.with(|m| {
            m.borrow_mut().remove(&wid);
        });
        WORKER_CLOSED.with(|g| {
            g.borrow_mut()
                .get_or_insert_with(Default::default)
                .insert(wid);
        });
        ns_emit(
            vm,
            Value::Object(ObjectRef(wid)),
            "exit",
            &[Value::Number(code as f64)],
        )?;
    }
    maybe_deactivate_real_worker_source(vm);
    Ok(progressed)
}

/// 无存活真实 worker 时注销事件源（事件循环停止为其泵询）。
fn maybe_deactivate_real_worker_source(vm: &mut Vm) {
    let empty = REAL_WORKERS.with(|m| m.borrow().is_empty());
    if empty {
        vm.deactivate_event_source("real_workers");
    }
}

/// worker 线程全局注入（真实线程路径；装配层在 `run_module` 前调用）：
/// `parentPort`（通道端口）、`isMainThread=false`、`workerData`。
pub fn setup_worker_globals(vm: &mut Vm) {
    let Some(io) = crate::worker::worker_thread_io() else {
        return;
    };
    let pp = make_port(vm, "worker_threads:parent_port");
    REAL_PP_ID.with(|c| *c.borrow_mut() = Some(pp.0));
    let pp_val = Value::Object(pp);
    vm.globals.insert("parentPort".to_owned(), pp_val);
    vm.globals
        .insert("isMainThread".to_owned(), Value::Boolean(false));
    let mut worker_data_val = Value::Null;
    if let Some(json) = io.worker_data_json.as_deref() {
        if let Ok(v) = json_string_to_value(vm, json) {
            worker_data_val = v;
            vm.globals.insert("workerData".to_owned(), worker_data_val);
        }
    }
    // 同步已构建的模块单例表面（register_all 引导期即已 build，缓存对象
    // 不会重读全局——对齐伪 worker 的 restore 逻辑）
    if let Some(m) = vm.builtin_registry.module("worker_threads") {
        let target = Value::Object(m);
        let _ = vm.set_property(target, "isMainThread", Value::Boolean(false));
        let _ = vm.set_property(target, "parentPort", pp_val);
        let _ = vm.set_property(target, "workerData", worker_data_val);
        let _ = vm.set_property(target, "threadId", Value::Number(io.thread_id as f64));
    }
}

/// worker 线程事件循环（模块体执行完后调用；直至无待办工作 / terminate）。
/// 返回退出码：0 正常；1 terminate。
pub fn run_worker_event_loop(vm: &mut Vm) -> u32 {
    let Some(io) = crate::worker::worker_thread_io() else {
        return 0;
    };
    loop {
        if io.terminate.load(std::sync::atomic::Ordering::SeqCst) {
            return 1;
        }
        let _ = vm.drain_microtasks();
        flush_worker_stdout(vm);
        // 非阻塞收取主线程信封（parentPort 消息 / workerMessage 投递 / ack 回程）
        while let Ok(msg) = io.from_main.try_recv() {
            match msg {
                crate::worker::WorkerInbound::PortMessage(json) => {
                    deliver_main_message(vm, &json);
                }
                crate::worker::WorkerInbound::WorkerMessage {
                    request_id,
                    source,
                    json,
                } => {
                    deliver_worker_message(vm, request_id, source, &json);
                }
                crate::worker::WorkerInbound::RouteAck { request_id, result } => {
                    let pending = ROUTE_PENDING_WORKER.with(|g| g.borrow_mut().remove(&request_id));
                    if let Some(pending) = pending {
                        let _ = settle_route(vm, pending, result);
                    }
                }
            }
        }
        if io.terminate.load(std::sync::atomic::Ordering::SeqCst) {
            return 1;
        }
        // 有本地待办（定时器 / 事件源如子进程完成 / **parentPort 挂
        // 'message' 监听器**）：排空一轮后重查 terminate 旗标与消息，随后
        // 再判空。Node 语义：消息端口挂监听器即保活 worker——此前仅按
        // 宏任务/事件源判定，纯消息应答型 worker（无定时器）会误退出。
        let pp_waiting = REAL_PP_ID.with(|c| {
            c.borrow()
                .is_some_and(|pp| ns_listener_count(pp, "message") > 0)
        });
        if !vm.macro_tasks.is_empty() || vm.has_active_event_sources() || pp_waiting {
            let _ = vm.drain_macro_tasks();
            flush_worker_stdout(vm);
            if pp_waiting && vm.macro_tasks.is_empty() && !vm.has_active_event_sources() {
                // 纯监听等待（仅 parentPort 挂 'message' 保活，无宏任务/事件源）：
                // 1ms 让出 CPU 防忙轮询。此条件原先写反——挂监听时反而不睡，
                // 长驻应答型 worker 会单核 100% 空转。
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            continue;
        }
        // 无待办：worker 事件循环已排空——正常退出（对齐 Node：模块体结束
        // 且无挂起句柄时 worker 退出；此后的主线程消息按失活端口丢弃）
        return 0;
    }
}

/// 把主线程消息派发到 worker 侧 parentPort 的 'message' 监听器。
fn deliver_main_message(vm: &mut Vm, json: &str) {
    let Some(pp) = REAL_PP_ID.with(|c| *c.borrow()) else {
        return;
    };
    if let Ok(val) = json_string_to_value(vm, json) {
        let _ = ns_emit(vm, Value::Object(ObjectRef(pp)), "message", &[val]);
    }
}

/// worker 侧 `workerMessage` 投递（`postMessageToThread` 通道）：反序列化值，
/// 对本线程 process 派发 `'workerMessage'(value, source)`，并把投递结果
/// 以 ack 回送主线程（Node `receiveMessageFromWorker` 响应口径）。
fn deliver_worker_message(vm: &mut Vm, request_id: u64, source: u64, json: &str) {
    let result = match vm.process_object {
        Some(process) if ns_listener_count(process.0, "workerMessage") > 0 => {
            match json_string_to_value(vm, json) {
                Ok(val) => {
                    match ns_emit(
                        vm,
                        Value::Object(process),
                        "workerMessage",
                        &[val, Value::Number(source as f64)],
                    ) {
                        Ok(()) => crate::worker::ROUTE_DELIVERED,
                        Err(VmError::Thrown(_)) => crate::worker::ROUTE_LISTENER_ERROR,
                        Err(_) => crate::worker::ROUTE_LISTENER_ERROR,
                    }
                }
                Err(_) => crate::worker::ROUTE_LISTENER_ERROR,
            }
        }
        _ => crate::worker::ROUTE_NO_LISTENERS,
    };
    if let Some(io) = crate::worker::worker_thread_io() {
        let _ = io.to_main.send(crate::worker::WorkerEvent::RouteAck {
            origin: source,
            request_id,
            result,
        });
    }
}

/// worker 线程 stdout 刷盘：各 Vm 独立缓冲，泵间隙写真实 stdout。
fn flush_worker_stdout(vm: &mut Vm) {
    if vm.stdout_records.is_empty() {
        return;
    }
    let lines: Vec<String> = vm.stdout_records.drain(..).collect();
    use std::io::Write as _;
    let mut out = std::io::stdout();
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
}

/// GC 根快照：端口消息队列、广播队列、worker 环境表与投递挂起表
/// （线程局部静态持有）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    PORT_STATES.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for st in map.values() {
                for v in &st.queue {
                    out.push(*v);
                }
            }
        }
    });
    ENV_DATA.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for v in map.values() {
                out.push(*v);
            }
        }
    });
    ROUTE_PENDING_MAIN.with(|g| {
        for p in g.borrow().values() {
            out.push(p.resolver);
            out.push(p.reject);
        }
    });
    ROUTE_PENDING_WORKER.with(|g| {
        for p in g.borrow().values() {
            out.push(p.resolver);
            out.push(p.reject);
        }
    });
}
