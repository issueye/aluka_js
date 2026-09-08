//! 真实跨物理线程 worker 的传输基建（M5.1）。
//!
//! 分层约束（AGENTS.md）：本模块只定义**纯数据传输契约**——消息通道端点、
//! 线程角色标记与 worker 线程的 spawn 钩子签名；JS 源码的编译能力留在
//! `aluka-runtime` 装配层（经 [`WorkerEntryFn`] 钩子注入），VM 层不依赖编译器。
//!
//! 线程模型：每个线程独享一个 `Vm` 实例（堆、内置静态表均线程局部），
//! 跨线程只传 JSON 字符串（`postMessage` 结构化克隆的近似）：
//! - 主线程：`new Worker(path)` 经 `Vm.worker_entry` 钩子 spawn 物理线程，
//!   拿到 [`WorkerBridge`]（通道端点 + 终止旗标）；
//! - worker 线程：运行时层先 [`set_worker_thread_io`]，编译并执行 worker
//!   文件，随后 [`run_worker_event_loop`] 泵事件直至退出。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};

/// 主线程 → worker 线程消息载荷（JSON 序列化的值）。
pub type WorkerInbound = String;

/// worker 线程 → 主线程事件。
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// `parentPort.postMessage(value)`（JSON 序列化值）
    Message(String),
    /// worker 未捕获异常（已格式化文本）
    Error(String),
    /// worker 退出码（0 正常结束；1 异常或 terminate）
    Exit(u32),
}

/// 主线程侧桥：真实 worker 线程的通道端点与终止旗标。
pub struct WorkerBridge {
    /// 物理线程 id（对齐 Node `worker.threadId`，自 1 起）
    pub thread_id: u64,
    /// 主线程 → worker 消息发送端
    pub to_worker: Sender<WorkerInbound>,
    /// worker → 主线程事件接收端
    pub from_worker: Receiver<WorkerEvent>,
    /// 终止旗标：`terminate()` 置位，worker 事件循环轮询后退出
    pub terminate: Arc<AtomicBool>,
}

/// worker 线程 spawn 钩子：由装配层（`aluka-runtime`）实现——编译并运行
/// worker JS 文件。返回主线程侧桥。
pub type WorkerEntryFn = dyn Fn(&str, Option<&str>) -> Result<WorkerBridge, String> + Send + Sync;

/// worker 线程的 I/O 束（spawn 钩子在 worker 线程内构建并登记）。
pub struct WorkerThreadIo {
    /// 物理线程 id
    pub thread_id: u64,
    /// `new Worker(path, { workerData })` 的 JSON 序列化值
    pub worker_data_json: Option<String>,
    /// worker → 主线程事件发送端
    pub to_main: Sender<WorkerEvent>,
    /// 主线程 → worker 消息接收端
    pub from_main: Receiver<WorkerInbound>,
    /// 终止旗标
    pub terminate: Arc<AtomicBool>,
}

thread_local! {
    /// 本线程的 worker 角色标记（主线程为 None；束仅本线程访问，故 Rc 足矣）。
    static WORKER_THREAD_IO: RefCell<Option<Rc<WorkerThreadIo>>> = const { RefCell::new(None) };
}

/// 登记 worker 线程 I/O 束（worker 线程启动早期由 spawn 钩子调用）。
pub fn set_worker_thread_io(io: WorkerThreadIo) {
    WORKER_THREAD_IO.with(|c| *c.borrow_mut() = Some(Rc::new(io)));
}

/// 读取本线程 worker I/O 束（非 worker 线程返回 None）。
pub fn worker_thread_io() -> Option<Rc<WorkerThreadIo>> {
    WORKER_THREAD_IO.with(|c| c.borrow().clone())
}

/// 本线程是否为 worker 线程（决定 `worker_threads` 模块表面）。
pub fn is_worker_thread() -> bool {
    worker_thread_io().is_some()
}
