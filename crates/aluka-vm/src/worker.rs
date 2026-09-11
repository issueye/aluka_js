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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};

/// `postMessageToThread` 投递结果码（对齐 Node `receiveMessageFromWorker` 响应）。
pub const ROUTE_DELIVERED: u8 = 0;
/// 投递结果码：目标线程或监听器缺失。
pub const ROUTE_NO_LISTENERS: u8 = 1;
/// 投递结果码：目标监听器执行时抛错。
pub const ROUTE_LISTENER_ERROR: u8 = 2;

/// worker 载荷来源（`new Worker` 第一参数的解释形态，M5.1 eval worker）。
#[derive(Debug, Clone)]
pub enum WorkerSource {
    /// 模块文件路径（默认形态）
    File(String),
    /// JS 源码串（`{ eval: true }`；Node `__filename` 为 `[worker eval]`）
    Eval(String),
}

/// 主线程 → worker 线程载荷信封（原为裸 JSON 串；`postMessageToThread`
/// 通道引入投递请求与 ack 回程后升级为信封枚举）。
pub enum WorkerInbound {
    /// worker 端 `parentPort` 的 `'message'`（原形态：结构化克隆字节 base64）
    PortMessage(String),
    /// `postMessageToThread` 投递：在 worker 侧对 process 派发
    /// `'workerMessage'`（`source` 为原始发送方线程 id，ack 回程据此路由）
    WorkerMessage {
        /// 投递请求 id（发起方挂起表键）
        request_id: u64,
        /// 原始发送方线程 id
        source: u64,
        /// 结构化克隆值（base64 字节串）
        json: String,
    },
    /// `postMessageToThread` ack 回程：投递结果（结果码见 [`ROUTE_DELIVERED`] 等）
    RouteAck {
        /// 投递请求 id（发起方挂起表键）
        request_id: u64,
        /// 投递结果码
        result: u8,
    },
}

/// worker 线程 → 主线程事件。
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// `parentPort.postMessage(value)`（JSON 序列化值）
    Message(String),
    /// worker 未捕获异常（错误名 + 消息；主线程 `'error'` 收到 Error 对象）
    Error {
        /// 错误名（`TypeError`/`SyntaxError`/`Error`…）
        name: String,
        /// 错误消息文本
        message: String,
    },
    /// worker 退出码（0 正常结束；1 异常或 terminate）
    Exit(u32),
    /// `postMessageToThread` 投递请求（worker 侧发起）：`destination` 可为
    /// 0（主线程）或其他 worker——一律经主线程中转/投递。
    RouteRequest {
        /// 投递请求 id（发起方挂起表键）
        request_id: u64,
        /// 原始发送方线程 id
        source: u64,
        /// 目标线程 id（0 = 主线程）
        destination: u64,
        /// 结构化克隆值（base64 字节串）
        json: String,
    },
    /// 投递结果的 ack 回程（worker 侧对收到的 [`WorkerInbound::WorkerMessage`]
    /// 的响应；`origin` = 原始发送方线程 id，主线程据此路由回程）
    RouteAck {
        /// 原始发送方线程 id（0 = 主线程自己的请求）
        origin: u64,
        /// 投递请求 id
        request_id: u64,
        /// 投递结果码
        result: u8,
    },
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

impl WorkerBridge {
    /// 投递主线程 → worker 载荷信封。
    pub fn send(&self, msg: WorkerInbound) {
        let _ = self.to_worker.send(msg);
    }
}

/// worker 线程 spawn 钩子：由装配层（`aluka-runtime`）实现——编译并运行
/// worker（文件路径或 eval 源码串）。返回主线程侧桥。
pub type WorkerEntryFn =
    dyn Fn(WorkerSource, Option<&str>) -> Result<WorkerBridge, String> + Send + Sync;

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

/// `postMessageToThread` 投递请求 id 计数（进程级唯一；回程按
/// `(origin, request_id)` 路由，全局唯一便于排查）。
static ROUTE_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 分配下一个投递请求 id。
pub fn next_route_request_id() -> u64 {
    ROUTE_REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst) + 1
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
