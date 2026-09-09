//! 进程类内置模块（`child_process` / `worker_threads` / `cluster`）的共享基建。
//!
//! 对齐 Node.js 22 LTS 标准 的两套机制：
//! - **实例事件器**（`nodeevents.NewEmitterInstance`）：`_builtinNs` 命名空间
//!   实例的 `on/once/emit/off/...` 通用实现与监听器存储，[`ns_emit`] 同时供
//!   事件泵在 VM 线程派发事件；`error` 事件无监听器时抛原值（Go emit 特殊路径）；
//! - **proc 事件泵**（goroutine + PostTask 模型）：后台线程（读管道 / 收集
//!   exec 输出）把事件推入 [`PROC_EVENTS`] 队列，`proc` 事件源在宏任务排空后
//!   轮询派发，FIFO 顺序对齐 Go 的 PostTask 队列。

use crate::builtins::{BuiltinRegistry, current_receiver, register_handler};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;

// ---------------------------------------------------------------------------
// 共享实例事件器
// ---------------------------------------------------------------------------

/// 实例事件器监听器条目：回调 + 是否一次性。
#[derive(Debug, Clone)]
pub(crate) struct NsListener {
    /// 监听器回调
    pub callback: Value,
    /// 是否为 once 注册（触发一次后自删）
    pub once: bool,
}

/// 实例事件器状态（对象句柄 id → 监听器表）。
#[derive(Debug, Default)]
struct NsEmitterState {
    /// 事件名 → 监听器列表
    listeners: HashMap<String, Vec<NsListener>>,
}

// 全部 `_builtinNs` 实例的监听器状态（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static NS_EMITTERS: std::cell::RefCell<Option<HashMap<u32, NsEmitterState>>> =
        const { std::cell::RefCell::new(None) };
}

fn with_ns_state<F, R>(id: u32, f: F) -> R
where
    F: FnOnce(&mut NsEmitterState) -> R,
{
    NS_EMITTERS.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(HashMap::new);
        f(map.entry(id).or_default())
    })
}

/// 给实例对象挂 `_builtinNs` 命名空间与方法原生函数属性（属性名 `{ns}.{m}`，
/// `CALL_METHOD` 经命名空间分派命中）。
pub(crate) fn ns_attach(vm: &mut Vm, obj: ObjectRef, ns: &'static str, methods: &[&str]) {
    let ns_val = vm.alloc_string(ns.to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns_val));
    for m in methods {
        let f = vm.alloc_native_fn(&format!("{ns}.{m}"));
        let _ = vm.set_property(Value::Object(obj), m, Value::Object(f));
    }
}

/// 实例事件器的通用方法清单（各命名空间按需子集挂属性）。
pub(crate) const EMITTER_METHODS: &[&str] = &[
    "on",
    "addListener",
    "once",
    "off",
    "removeListener",
    "removeAllListeners",
    "emit",
    "listenerCount",
];

/// 把共享实例事件器的通用处理器登记到 `ns` 命名空间键下。
pub(crate) fn register_ns_emitter_handlers(registry: &mut BuiltinRegistry, ns: &str) {
    register_handler(registry, ns, "on", inst_on);
    register_handler(registry, ns, "addListener", inst_on);
    register_handler(registry, ns, "once", inst_once);
    register_handler(registry, ns, "off", inst_off);
    register_handler(registry, ns, "removeListener", inst_off);
    register_handler(registry, ns, "removeAllListeners", inst_remove_all);
    register_handler(registry, ns, "emit", inst_emit);
    register_handler(registry, ns, "listenerCount", inst_listener_count);
}

/// `inst.on(event, cb)` / `addListener`。
fn inst_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() < 2 {
        return Ok(receiver);
    }
    let event = vm.to_property_key(args[0]);
    let cb = args[1];
    if let Value::Object(_) = cb {
        with_ns_state(r.0, |s| {
            s.listeners.entry(event).or_default().push(NsListener {
                callback: cb,
                once: false,
            });
        });
    }
    Ok(receiver)
}

/// `inst.once(event, cb)`。
fn inst_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() < 2 {
        return Ok(receiver);
    }
    let event = vm.to_property_key(args[0]);
    let cb = args[1];
    if let Value::Object(_) = cb {
        with_ns_state(r.0, |s| {
            s.listeners.entry(event).or_default().push(NsListener {
                callback: cb,
                once: true,
            });
        });
    }
    Ok(receiver)
}

/// `inst.off(event, cb)` / `removeListener`。
fn inst_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() < 2 {
        return Ok(receiver);
    }
    let event = vm.to_property_key(args[0]);
    let cb = args[1];
    with_ns_state(r.0, |s| {
        if let Some(list) = s.listeners.get_mut(&event) {
            if let Some(pos) = list.iter().position(|l| same_object(l.callback, cb)) {
                list.remove(pos);
            }
        }
    });
    Ok(receiver)
}

/// 监听器回调的同一性比较（同一堆句柄）。
fn same_object(a: Value, b: Value) -> bool {
    matches!((a, b), (Value::Object(x), Value::Object(y)) if x == y)
}

/// `inst.removeAllListeners([event])`。
fn inst_remove_all(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    match args.first() {
        Some(v) if !matches!(v, Value::Undefined) => {
            let event = vm.to_property_key(*v);
            with_ns_state(r.0, |s| {
                s.listeners.remove(&event);
            });
        }
        _ => with_ns_state(r.0, |s| s.listeners.clear()),
    }
    Ok(receiver)
}

/// `inst.emit(event, ...args)`：`error` 事件无监听器时抛原值。
fn inst_emit(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(event_val) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let event = vm.to_property_key(*event_val);
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
    ns_emit(vm, receiver, &event, &emit_args)?;
    Ok(Value::Boolean(true))
}

/// `inst.listenerCount(event)`。
fn inst_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Number(0.0));
    };
    let event = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    Ok(Value::Number(ns_listener_count(r.0, &event) as f64))
}

/// 事件名对应的监听器数量。
pub(crate) fn ns_listener_count(id: u32, event: &str) -> usize {
    NS_EMITTERS.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&id))
            .and_then(|s| s.listeners.get(event))
            .map(Vec::len)
            .unwrap_or(0)
    })
}

/// 给 `_builtinNs` 实例追加一个 Rust 侧监听器（内部事件转接用，如 cluster
/// 把 child 的 'exit'/'message' 转接到 Worker 包装对象；回调为已登记分派表
/// 的原生函数名对应的 NativeFn 值）。
pub(crate) fn ns_push_listener(id: u32, event: &str, callback: Value) {
    with_ns_state(id, |s| {
        s.listeners
            .entry(event.to_owned())
            .or_default()
            .push(NsListener {
                callback,
                once: false,
            });
    });
}

/// 在实例上派发事件：快照监听器（once 自删）后逐个调用；`error` 事件无监听器
/// 时返回 `Err(Thrown(首参))`（Go EmitEvent + emit 特殊路径一致）。
pub(crate) fn ns_emit(
    vm: &mut Vm,
    target: Value,
    event: &str,
    args: &[Value],
) -> Result<(), VmError> {
    let Value::Object(r) = target else {
        return Ok(());
    };
    let to_call: Vec<Value> = with_ns_state(r.0, |s| {
        let Some(list) = s.listeners.get_mut(event) else {
            return Vec::new();
        };
        let mut call = Vec::new();
        let mut keep = Vec::with_capacity(list.len());
        for l in list.drain(..) {
            call.push(l.callback);
            if !l.once {
                keep.push(l);
            }
        }
        *list = keep;
        call
    });
    if to_call.is_empty() {
        if event == "error" {
            let err_val = args.first().copied().unwrap_or(Value::Undefined);
            return Err(VmError::Thrown(err_val));
        }
        return Ok(());
    }
    for cb in to_call {
        vm.invoke_callable(cb, target, args)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// proc 事件队列与子进程表
// ---------------------------------------------------------------------------

/// 读管道线程写入的数据流种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamKind {
    /// 标准输出管道
    Stdout,
    /// 标准错误管道
    Stderr,
}

/// 子进程/执行任务的待派发事件（跨线程生产，VM 线程消费）。
#[derive(Debug)]
pub(crate) enum ProcEvent {
    /// 子进程已启动（'spawn' 事件）
    Spawn {
        /// 子进程对象句柄 id
        child: u32,
    },
    /// 子进程启动失败（'error' 事件 + 'exit'(-1)，携带 Go 风格错误串）
    SpawnError {
        /// 子进程对象句柄 id
        child: u32,
        /// Go 风格错误串
        message: String,
    },
    /// 管道读到一块数据（stream 'data' 事件，Buffer 载荷）
    Data {
        /// 数据流对象句柄 id
        stream: u32,
        /// 数据字节
        chunk: Vec<u8>,
    },
    /// 管道读到 EOF（子进程 EOF 标记 + 流属性翻转 + 'end'/'close' 事件）
    StreamEof {
        /// 所属子进程 id
        child: u32,
        /// 数据流对象句柄 id
        stream: u32,
        /// 数据流种类
        kind: StreamKind,
    },
    /// 子进程退出（'exit' + 'close' 事件）
    Exit {
        /// 子进程对象句柄 id
        child: u32,
        /// 退出码
        code: i32,
    },
    /// exec/execFile 完成（调用回调 `(err, stdout, stderr)`）
    ExecDone {
        /// JS 回调
        cb: Value,
        /// 错误串（无错为 None → 回调首参 null）
        err: Option<String>,
        /// 收集的 stdout
        stdout: String,
        /// 收集的 stderr
        stderr: String,
    },
    /// 运行 worker 模块体（worker_threads 伪 worker：加载并执行 worker 脚本）
    RunWorker {
        /// 主线程侧 Worker 对象句柄 id
        worker: u32,
        /// 模块路径（Go loader.Run 语义）
        path: String,
        /// 经 JSON 往返的 workerData
        data: Option<Value>,
        /// eval 模式（Rust 字节码 VM 不支持，走 error 路径）
        eval: bool,
    },
    /// worker → 主线程消息（worker 'message' 事件）
    WorkerToMain {
        /// Worker 对象句柄 id
        worker: u32,
        /// 消息值
        msg: Value,
    },
    /// 主线程 → worker parentPort 消息（缓冲到端口队列后按需派发）
    MainToWorker {
        /// parentPort 端口对象句柄 id
        pp: u32,
        /// 消息值
        msg: Value,
    },
    /// 端口缓冲消息派发（'message' 事件，携带同步搬出的全部消息）
    PortDeliver {
        /// 端口对象句柄 id
        port: u32,
        /// 待派发消息
        msgs: Vec<Value>,
    },
    /// worker 事件循环结束（worker 'exit' 事件）
    WorkerExit {
        /// Worker 对象句柄 id
        worker: u32,
        /// 退出码
        code: i32,
    },
    /// worker 加载/运行失败（worker 'error' 事件）
    WorkerError {
        /// Worker 对象句柄 id
        worker: u32,
        /// Go 风格错误串
        message: String,
    },
}

/// 跨线程事件队列（进程级）：条目携带属主线程 id——事件回调的堆句柄属于
/// 发起线程的 Vm 堆，泵（属主线程）只派发属主匹配的事件，其余线程的事件
/// 留在队列由其属主泵消费（真实 worker 线程隔离的前提）。
static PROC_EVENTS: std::sync::Mutex<Option<VecDeque<(std::thread::ThreadId, ProcEvent)>>> =
    std::sync::Mutex::new(None);

/// 推入一个待派发事件（任意线程可调用；属主为当前线程）。
pub(crate) fn push_event(ev: ProcEvent) {
    push_event_for(std::thread::current().id(), ev);
}

/// 以指定属主线程推入事件（辅助线程回投时须传发起线程 id）。
pub(crate) fn push_event_for(owner: std::thread::ThreadId, ev: ProcEvent) {
    let mut guard = PROC_EVENTS.lock().unwrap();
    guard
        .get_or_insert_with(VecDeque::new)
        .push_back((owner, ev));
}

/// 弹出队首属主匹配的事件（其余线程事件保留原序）。
fn pop_event() -> Option<ProcEvent> {
    let me = std::thread::current().id();
    let mut guard = PROC_EVENTS.lock().unwrap();
    let queue = guard.as_mut()?;
    let idx = queue.iter().position(|(owner, _)| *owner == me)?;
    queue.remove(idx).map(|(_, ev)| ev)
}

/// 子进程运行期状态（对象句柄 id → 状态）。
pub(crate) struct ChildState {
    /// OS 子进程句柄（kill / try_wait 用）
    pub child: std::process::Child,
    /// stdout 管道是否已 EOF（inherit 模式初始即为 true）
    pub stdout_eof: bool,
    /// stderr 管道是否已 EOF（inherit 模式初始即为 true）
    pub stderr_eof: bool,
    /// 是否已入队退出事件
    pub exit_enqueued: bool,
}

// 子进程表（按句柄 id 有序，保证多子进程时派发顺序确定；线程局部）。
thread_local! {
    static CHILDREN: std::cell::RefCell<Option<BTreeMap<u32, ChildState>>> =
        const { std::cell::RefCell::new(None) };
}

/// 在子进程表中执行闭包。
pub(crate) fn with_children<F, R>(f: F) -> R
where
    F: FnOnce(&mut BTreeMap<u32, ChildState>) -> R,
{
    CHILDREN.with(|g| {
        let mut binding = g.borrow_mut();
        let map = binding.get_or_insert_with(BTreeMap::new);
        f(map)
    })
}

/// 已 spawn 成功、等待退出事件入队，或事件队列非空 → 事件源保持活跃。
pub(crate) fn proc_source_busy() -> bool {
    let me = std::thread::current().id();
    let events_left = PROC_EVENTS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|q| q.iter().any(|(owner, _)| *owner == me));
    let children_left = CHILDREN.with(|g| g.borrow().as_ref().is_some_and(|m| !m.is_empty()));
    events_left || children_left || EXEC_PENDING.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// 在途 exec/execFile 后台任务数（防泵过早休眠：线程启动到事件入队之间存在
/// 空窗，事件源据此保持活跃）。
static EXEC_PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// exec 后台任务记账：进入时 +1（返回减量闭包由线程收尾调用）。
pub(crate) fn begin_exec_task() -> impl FnOnce() + Send {
    EXEC_PENDING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    move || {
        EXEC_PENDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// `proc` 事件源泵：把队列中的事件按 FIFO 派发为 JS 事件 / 回调。
/// 返回本轮是否有进展。
pub(crate) fn pump_proc(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    loop {
        enqueue_ready_exits();
        let Some(ev) = pop_event() else {
            break;
        };
        dispatch_proc_event(vm, ev)?;
        progressed = true;
    }
    if !proc_source_busy() {
        vm.deactivate_event_source("proc");
    }
    Ok(progressed)
}

/// 扫描子进程表：两条管道都已 EOF 且进程已退出的，入队 'exit' 事件
/// （保证 Data/End/Close 先于 Exit 的确定顺序，对齐 Go 观测行为）。
fn enqueue_ready_exits() {
    let mut to_exit: Vec<(u32, i32)> = Vec::new();
    with_children(|map| {
        for (id, st) in map.iter_mut() {
            if st.exit_enqueued {
                continue;
            }
            let pipes_done = st.stdout_eof && st.stderr_eof;
            if !pipes_done {
                continue;
            }
            if let Ok(Some(status)) = st.child.try_wait() {
                let code = status.code().unwrap_or(-1);
                st.exit_enqueued = true;
                to_exit.push((*id, code));
            }
        }
    });
    for (id, code) in to_exit {
        push_event(ProcEvent::Exit { child: id, code });
    }
}

/// 派发单个 proc 事件到 JS 层。
fn dispatch_proc_event(vm: &mut Vm, ev: ProcEvent) -> Result<(), VmError> {
    match ev {
        ProcEvent::Spawn { child } => ns_emit(vm, Value::Object(ObjectRef(child)), "spawn", &[]),
        ProcEvent::SpawnError { child, message } => {
            let target = Value::Object(ObjectRef(child));
            let msg = vm.alloc_string(message);
            ns_emit(vm, target, "error", &[Value::Object(msg)])?;
            ns_emit(vm, target, "exit", &[Value::Number(-1.0), Value::Null])
        }
        ProcEvent::Data { stream, chunk } => {
            let target = Value::Object(ObjectRef(stream));
            let buf = crate::builtins::buffer::create_buffer_instance(vm, chunk);
            ns_emit(vm, target, "data", &[Value::Object(buf)])
        }
        ProcEvent::StreamEof {
            child,
            stream,
            kind,
        } => {
            // 先标记该管道已 EOF（退出事件入队的前置条件），再翻转流属性并派发。
            with_children(|map| {
                if let Some(st) = map.get_mut(&child) {
                    match kind {
                        StreamKind::Stdout => st.stdout_eof = true,
                        StreamKind::Stderr => st.stderr_eof = true,
                    }
                }
            });
            let target = Value::Object(ObjectRef(stream));
            let _ = vm.set_property(target, "readable", Value::Boolean(false));
            let _ = vm.set_property(target, "readableEnded", Value::Boolean(true));
            ns_emit(vm, target, "end", &[])?;
            ns_emit(vm, target, "close", &[])
        }
        ProcEvent::Exit { child, code } => {
            let target = Value::Object(ObjectRef(child));
            ns_emit(
                vm,
                target,
                "exit",
                &[Value::Number(code as f64), Value::Null],
            )?;
            ns_emit(
                vm,
                target,
                "close",
                &[Value::Number(code as f64), Value::Null],
            )?;
            with_children(|map| {
                map.remove(&child);
            });
            Ok(())
        }
        ProcEvent::ExecDone {
            cb,
            err,
            stdout,
            stderr,
        } => {
            let err_val = match err {
                None => Value::Null,
                Some(msg) => Value::Object(vm.alloc_string(msg)),
            };
            let out = vm.alloc_string(stdout);
            let err_s = vm.alloc_string(stderr);
            vm.invoke_callable(
                cb,
                Value::Undefined,
                &[err_val, Value::Object(out), Value::Object(err_s)],
            )?;
            Ok(())
        }
        ProcEvent::RunWorker {
            worker,
            path,
            data,
            eval,
        } => crate::builtins::worker_threads::run_worker_body(vm, worker, path, data, eval),
        ProcEvent::WorkerToMain { worker, msg } => {
            let target = Value::Object(ObjectRef(worker));
            ns_emit(vm, target, "message", &[msg])
        }
        ProcEvent::MainToWorker { pp, msg } => {
            let target = Value::Object(ObjectRef(pp));
            crate::builtins::worker_threads::port_post(vm, target, msg)
        }
        ProcEvent::PortDeliver { port, msgs } => {
            let target = Value::Object(ObjectRef(port));
            for m in msgs {
                crate::builtins::worker_threads::port_emit(vm, target, m)?;
            }
            Ok(())
        }
        ProcEvent::WorkerExit { worker, code } => {
            let target = Value::Object(ObjectRef(worker));
            ns_emit(vm, target, "exit", &[Value::Number(code as f64)])
        }
        ProcEvent::WorkerError { worker, message } => {
            let target = Value::Object(ObjectRef(worker));
            let msg = vm.alloc_string(message);
            ns_emit(vm, target, "error", &[Value::Object(msg)])
        }
    }
}

/// 读管道线程：读到一块推一条 Data 事件，EOF 推 StreamEof。
pub(crate) fn spawn_pipe_reader<R: Read + Send + 'static>(
    mut pipe: R,
    child_id: u32,
    stream_id: u32,
    kind: StreamKind,
) {
    // 属主线程 id：读管道线程回投事件须标记发起线程（堆句柄属主）
    let owner = std::thread::current().id();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    push_event_for(
                        owner,
                        ProcEvent::Data {
                            stream: stream_id,
                            chunk: buf[..n].to_vec(),
                        },
                    );
                }
                Err(_) => break,
            }
        }
        push_event_for(
            owner,
            ProcEvent::StreamEof {
                child: child_id,
                stream: stream_id,
                kind,
            },
        );
    });
}

/// GC 根快照：`_builtinNs` 实例事件器的监听器回调（漏登记 = major 回收悬垂）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    NS_EMITTERS.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for state in map.values() {
                for listeners in state.listeners.values() {
                    for l in listeners {
                        out.push(l.callback);
                    }
                }
            }
        }
    });
}
