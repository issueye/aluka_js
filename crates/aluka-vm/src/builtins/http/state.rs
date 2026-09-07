//! `http` 内置库的 Rust 侧运行时状态。
//!
//! 与模板（`stream.rs`/`events.rs`）一致：所有 socket 与实例状态放在
//! `Mutex` 静态表里，以对象堆句柄 `ObjectRef.0` 为键；泵函数非阻塞地
//! accept/读写 socket，解析出完整报文后经 `vm.invoke_callable` 派发回调。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

/// 一条已接受的 TCP 连接（服务端）。
pub(crate) struct Conn {
    /// 连接内自增编号（跨 server 唯一，用于响应回写定位）
    pub id: u64,
    /// 非阻塞 socket
    pub stream: TcpStream,
    /// 读缓冲（未解析完的请求字节）
    pub buf: Vec<u8>,
    /// 未写出的响应字节（`WouldBlock` 时残留，下轮泵补写）
    pub out: Vec<u8>,
    /// 对端已关闭
    pub eof: bool,
    /// 是否有请求正处理中（响应写出前不再解析后续请求）
    pub res_active: bool,
}

/// 一个监听中的 HTTP 服务器。
pub(crate) struct Server {
    /// Server 对象堆句柄
    pub obj: u32,
    /// 非阻塞监听器（`close` 后置 None）
    pub listener: Option<TcpListener>,
    /// 监听地址（`address()` 展示用）
    pub host: String,
    /// 实际绑定端口（port=0 时为系统分配值）
    pub port: u16,
    /// 是否监听中
    pub listening: bool,
    /// 已接受的连接
    pub conns: Vec<Conn>,
}

/// 客户端请求阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// 待连接（泵内发起 `connect`）
    Connecting,
    /// 请求字节写出中
    Sending,
    /// 等待并解析响应
    AwaitingResponse,
    /// 已交付（终态）
    Done,
}

/// 一个进行中的客户端请求（`ClientRequest`）。
pub(crate) struct ClientReq {
    /// ClientRequest 对象堆句柄
    pub obj: u32,
    /// 请求方法
    pub method: String,
    /// 目标主机
    pub host: String,
    /// 目标端口
    pub port: u16,
    /// `Host` 头值（协议默认端口省略端口号，对齐 Go）
    pub host_header: String,
    /// 请求路径（RequestURI）
    pub path: String,
    /// 用户设置的请求头（保序）
    pub headers: Vec<(String, String)>,
    /// 累积的请求体
    pub body: Vec<u8>,
    /// 响应回调
    pub callback: Option<Value>,
    /// 是否已 end
    pub ended: bool,
    /// 是否已 abort/destroy
    pub aborted: bool,
    /// 错误是否已派发（防重复）
    pub error_dispatched: bool,
    /// 当前阶段
    pub stage: Stage,
    /// 连接 socket（连接成功后存在）
    pub stream: Option<TcpStream>,
    /// 读缓冲
    pub read_buf: Vec<u8>,
    /// 待写出的请求字节
    pub write_buf: Vec<u8>,
    /// 对端已关闭（响应未完整到达即断开）
    pub eof: bool,
}

/// `ServerResponse` 与底层连接的绑定（响应写出状态）。
pub(crate) struct RespBinding {
    /// 所属 Server 对象堆句柄
    pub server_obj: u32,
    /// 连接编号
    pub conn_id: u64,
    /// 状态码（`writeHead`/`statusCode` 属性可改）
    pub status: u16,
    /// 活动头表（`setHeader` 等实时可变，`getHeader` 读取处）
    pub live: Vec<(String, Vec<String>)>,
    /// 已冻结的线上头（`writeHead` 首次 flush 快照；None 表示未冻结）
    pub wire: Option<Vec<(String, Vec<String>)>>,
    /// 已缓冲响应体（`end` 时统一写出，对齐 Go 缓冲行为）
    pub body: Vec<u8>,
    /// 是否已 end
    pub finished: bool,
}

/// 解析请求后暂存的派发项（锁外构造 JS 对象并调用 handler）。
pub(crate) struct ReqDispatch {
    /// Server 对象堆句柄
    pub server_obj: u32,
    /// 连接编号
    pub conn_id: u64,
    /// 请求方法
    pub method: String,
    /// 请求目标
    pub target: String,
    /// JS 可见头（小写、剔除 host/transfer-encoding、补 content-length）
    pub headers: Vec<(String, Vec<String>)>,
    /// 请求体
    pub body: Vec<u8>,
}

static SERVERS: Mutex<Option<Vec<Server>>> = Mutex::new(None);
static CLIENTS: Mutex<Option<Vec<ClientReq>>> = Mutex::new(None);
static RESPONSES: Mutex<Option<HashMap<u32, RespBinding>>> = Mutex::new(None);
/// 监听器条目（回调 + 是否一次性）。
struct ListenerItem {
    /// 回调值
    callback: Value,
    /// 是否 once（发射后剔除）
    once: bool,
}

/// 监听器表：对象句柄 → (事件名 → 监听器列表)。
type ListenerMap = HashMap<u32, HashMap<String, Vec<ListenerItem>>>;

static LISTENERS: Mutex<Option<ListenerMap>> = Mutex::new(None);
static CONN_COUNTER: Mutex<u64> = Mutex::new(0);
/// 待发射事件队列（目标对象, 事件名）：`end` 的 finish/close、`listen` 的
/// listening 等，由泵在安全时机统一发射（对齐 Go `PostTask` 顺序）。
static PENDING_EVENTS: Mutex<Vec<(Value, &'static str)>> = Mutex::new(Vec::new());
/// 待发射 `'timeout'` 的请求对象队列（宏任务标记函数消费）。
static TIMEOUT_TARGETS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Agent keepAlive 连接池：origin("host:port") → 空闲 TCP 流（上限 4/origin，
/// 对齐 Node globalAgent keepAlive 默认语义；复用失败自动丢弃）。
pub(crate) static CONN_POOL: Mutex<Option<HashMap<String, Vec<std::net::TcpStream>>>> =
    Mutex::new(None);

/// 连接池单 origin 上限。
const POOL_CAP: usize = 4;

/// 从连接池取一条到 `origin` 的存活流（`peek` 探活：WouldBlock=存活，
/// Ok(0)=对端已关闭）。
pub(crate) fn pool_take(origin: &str) -> Option<std::net::TcpStream> {
    let mut guard = CONN_POOL.lock().unwrap();
    let pool = guard.get_or_insert_with(HashMap::new);
    while let Some(stream) = pool.get_mut(origin).and_then(|v| v.pop()) {
        let mut probe = [0u8; 1];
        match stream.peek(&mut probe) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Some(stream),
            _ => continue, // 对端已关或出错：丢弃继续取
        }
    }
    None
}

/// 归还一条流到连接池（超上限则丢弃）。
pub(crate) fn pool_put(origin: &str, stream: std::net::TcpStream) {
    let mut guard = CONN_POOL.lock().unwrap();
    let pool = guard.get_or_insert_with(HashMap::new);
    let slot = pool.entry(origin.to_owned()).or_default();
    if slot.len() < POOL_CAP {
        slot.push(stream);
    }
}

/// 入队一条待发射事件。
pub(crate) fn push_pending_event(target: Value, event: &'static str) {
    PENDING_EVENTS.lock().unwrap().push((target, event));
}

/// 取走全部待发射事件。
pub(crate) fn drain_pending_events() -> Vec<(Value, &'static str)> {
    std::mem::take(&mut *PENDING_EVENTS.lock().unwrap())
}

/// 入队 `'timeout'` 发射目标。
pub(crate) fn push_timeout_target(target: Value) {
    TIMEOUT_TARGETS.lock().unwrap().push(target);
}

/// 弹出队首 `'timeout'` 发射目标。
pub(crate) fn pop_timeout_target() -> Option<Value> {
    TIMEOUT_TARGETS.lock().unwrap().pop()
}

/// 可变借用服务器表。
pub(crate) fn with_servers<R>(f: impl FnOnce(&mut Vec<Server>) -> R) -> R {
    let mut guard = SERVERS.lock().unwrap();
    f(guard.get_or_insert_with(Vec::new))
}

/// 只读借用服务器表。
pub(crate) fn read_servers<R>(f: impl FnOnce(&[Server]) -> R) -> R {
    let guard = SERVERS.lock().unwrap();
    f(guard.as_deref().unwrap_or(&[]))
}

/// 可变借用客户端请求表。
pub(crate) fn with_clients<R>(f: impl FnOnce(&mut Vec<ClientReq>) -> R) -> R {
    let mut guard = CLIENTS.lock().unwrap();
    f(guard.get_or_insert_with(Vec::new))
}

/// 可变借用响应绑定表。
pub(crate) fn with_responses<R>(f: impl FnOnce(&mut HashMap<u32, RespBinding>) -> R) -> R {
    let mut guard = RESPONSES.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new))
}

/// 分配下一个连接编号。
pub(crate) fn next_conn_id() -> u64 {
    let mut guard = CONN_COUNTER.lock().unwrap();
    *guard += 1;
    *guard
}

/// 为实例对象注册事件监听器（`once` 由发射时剔除实现，存储为可迭代值表）。
pub(crate) fn add_listener(obj: u32, event: &str, cb: Value, once: bool) {
    let mut guard = LISTENERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.entry(obj)
        .or_default()
        .entry(event.to_string())
        .or_default()
        .push(ListenerItem { callback: cb, once });
}

/// 快照某事件当前监听器（剔除 `once` 项）。无监听器返回空表。
fn take_listeners(obj: u32, event: &str) -> Vec<Value> {
    let mut guard = LISTENERS.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return Vec::new();
    };
    let Some(entry) = map.get_mut(&obj) else {
        return Vec::new();
    };
    let Some(list) = entry.get_mut(event) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut keep = Vec::new();
    for item in list.drain(..) {
        out.push(item.callback);
        if !item.once {
            keep.push(item);
        }
    }
    *list = keep;
    out
}

/// 查询是否存在某事件的监听器。
pub(crate) fn has_listener(obj: u32, event: &str) -> bool {
    let guard = LISTENERS.lock().unwrap();
    guard
        .as_ref()
        .and_then(|m| m.get(&obj))
        .and_then(|e| e.get(event))
        .is_some_and(|l| !l.is_empty())
}

/// 移除某事件的指定回调监听器（首个匹配）。
pub(crate) fn remove_listener(obj: u32, event: &str, cb: Value) {
    let mut guard = LISTENERS.lock().unwrap();
    if let Some(list) = guard
        .as_mut()
        .and_then(|m| m.get_mut(&obj))
        .and_then(|e| e.get_mut(event))
    {
        if let Some(pos) = list.iter().position(|item| item.callback == cb) {
            list.remove(pos);
        }
    }
}

/// 在实例对象上发射事件：依次调用监听器；`error` 事件无监听器时按
/// EventEmitter 语义抛出（对齐 `events.rs` 与 Go `nodebase.EmitEvent`）。
pub(crate) fn emit(vm: &mut Vm, target: Value, event: &str, args: &[Value]) -> Result<(), VmError> {
    let Value::Object(r) = target else {
        return Ok(());
    };
    let cbs = take_listeners(r.0, event);
    if cbs.is_empty() {
        if event == "error" {
            let err = args.first().copied().unwrap_or(Value::Undefined);
            return Err(VmError::Thrown(err));
        }
        return Ok(());
    }
    for cb in cbs {
        vm.invoke_callable(cb, target, args)?;
    }
    Ok(())
}

/// 计算宏任务到期时间（与 `timers.rs::schedule_raw` 同一语义：
/// 追加到当前队列末尾的 `delay` 偏移之上），并压入队列。
pub(crate) fn schedule_task(vm: &mut Vm, cb: Value, delay: u64) {
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    vm.macro_tasks
        .push_back((id, last_due + delay, delay, cb, false));
}

/// 响应绑定快照：`(server_obj, conn_id, status, live, wire, body, finished)`。
pub(crate) type BindingSnapshot = (
    u32,
    u64,
    u16,
    Vec<(String, Vec<String>)>,
    Option<Vec<(String, Vec<String>)>>,
    Vec<u8>,
    bool,
);

/// 读取 ServerResponse 绑定（不存在返回 None 的克隆快照）。
pub(crate) fn response_binding(res_id: u32) -> Option<BindingSnapshot> {
    let guard = RESPONSES.lock().unwrap();
    guard.as_ref().and_then(|m| m.get(&res_id)).map(|b| {
        (
            b.server_obj,
            b.conn_id,
            b.status,
            b.live.clone(),
            b.wire.clone(),
            b.body.clone(),
            b.finished,
        )
    })
}

/// 更新响应绑定（存在时）。
pub(crate) fn update_response<R>(res_id: u32, f: impl FnOnce(&mut RespBinding) -> R) -> Option<R> {
    let mut guard = RESPONSES.lock().unwrap();
    guard.as_mut().and_then(|m| m.get_mut(&res_id)).map(f)
}
