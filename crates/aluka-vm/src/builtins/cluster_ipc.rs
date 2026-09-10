//! cluster / fork 子进程的 IPC 传输层（M5.2）。
//!
//! Node 的 IPC 通道是 `child_process.fork` 在 stdio 数组里追加的 **ipc 管道**
//! （fd 3，Windows 为命名管道，见 `lib/internal/child_process.js` 的
//! `getValidStdio` / `setupChannel`）。Rust 标准库无法在 spawn 时向子进程
//! 传递第 4 个 fd/管道句柄，因此本模块以**回环 TCP**（127.0.0.1 上 OS 分配的
//! 临时端口）承载同一条语义通道——对 JS 可见面（`process.send` /
//! `process.connected` / worker `message` 事件）与 Node 一致，仅传输介质不同：
//!
//! - **父进程侧**：每次 `cluster.fork` 绑定一个监听器（`spawn_worker_listener`），
//!   端口与一次性握手 key 经环境变量 `ALUKA_IPC_PORT` / `ALUKA_IPC_KEY`
//!   注入子进程；后台线程 accept 后校验握手帧，建立 worker id → 写端映射，
//!   随后持续按行读取并投递到本线程 inbox；
//! - **子进程侧**：`child_connect` 连接父进程监听器（阻塞 connect，失败即
//!   视为无 IPC 通道），后台线程读行投递 inbox；
//! - **帧格式**：一行一个 JSON 对象（与 Node 默认 `serialization: 'json'`
//!   的载荷语义一致，见 `lib/internal/child_process/serialization.js`）：
//!   `{"t":"m","v":<消息>}` / `{"t":"o"}`（online）/ `{"t":"l",...}`（listening）/
//!   `{"t":"d"}`（primary 发起的 disconnect）/ `{"t":"e"}`（exitedAfterDisconnect ack）；
//! - **跨线程边界只传字符串**：VM 堆句柄不可跨线程，inbox 条目一律是
//!   已解析前的原始行文本，由属主线程（发起 fork/连接的 VM 线程）在泵中
//!   解析为堆值（与 `proc_common` 的 proc 事件泵同一纪律）。
//!
//! 未实现（如实登记）：句柄（socket/server handle）传递（`sendHandle`）、
//! `serialization: 'advanced'`、以及 Node 的 `NODE_HANDLE` ack 协议。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// 父进程注入的 IPC 端口环境变量名。
pub(crate) const ENV_PORT: &str = "ALUKA_IPC_PORT";
/// 父进程注入的一次性握手 key 环境变量名。
pub(crate) const ENV_KEY: &str = "ALUKA_IPC_KEY";

/// 子进程通道在内部表中的 key（worker id 从 1 起，0 保留给子进程自身通道）。
const CHILD_CHANNEL_KEY: u64 = 0;

/// inbox 条目：一行文本或通道关闭。
#[derive(Debug)]
pub(crate) enum Incoming {
    /// 一行文本（JSON 帧，尚未解析）
    Line(String),
    /// 通道已关闭（对端 EOF / 本地关闭）
    Closed,
}

/// 跨线程 inbox（条目携带属主线程 id——堆值与事件派发都发生在属主 VM 线程）。
static INBOX: Mutex<Option<VecDeque<(ThreadId, u64, Incoming)>>> = Mutex::new(None);

/// 已关闭/已终结的通道（worker id → 关闭；0 = 子进程自身通道）。
static CLOSED_CHANNELS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// 父进程侧写端表（worker id → 与子进程连接的写端）。
static WRITERS: Mutex<Option<HashMap<u64, Arc<Mutex<TcpStream>>>>> = Mutex::new(None);

/// 握手 key 生成器（进程内单调计数，配合 pid + 纳秒时间戳）。
static KEY_SEQ: AtomicU64 = AtomicU64::new(0);

/// 读线程轮询粒度：读超时后回到循环顶部检查通道是否已被本地关闭。
const READ_POLL: Duration = Duration::from_millis(100);

/// accept 轮询粒度（非阻塞 accept + 小睡，便于通道关闭后线程退出）。
const ACCEPT_POLL: Duration = Duration::from_millis(5);

fn push_incoming(owner: ThreadId, key: u64, item: Incoming) {
    let mut guard = INBOX.lock().unwrap();
    guard
        .get_or_insert_with(VecDeque::new)
        .push_back((owner, key, item));
}

/// 取出队首属主为当前线程的 inbox 条目（其余线程条目保留原序）。
pub(crate) fn take_incoming() -> Option<(u64, Incoming)> {
    let me = std::thread::current().id();
    let mut guard = INBOX.lock().unwrap();
    let queue = guard.as_mut()?;
    let idx = queue.iter().position(|(owner, _, _)| *owner == me)?;
    queue.remove(idx).map(|(_, key, item)| (key, item))
}

/// 当前线程是否还有 inbox 条目（事件源活性判定用）。
pub(crate) fn inbox_pending() -> bool {
    let me = std::thread::current().id();
    INBOX
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|q| q.iter().any(|(owner, _, _)| *owner == me))
}

fn mark_closed(key: u64) {
    CLOSED_CHANNELS
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(key);
}

fn is_closed(key: u64) -> bool {
    CLOSED_CHANNELS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.contains(&key))
}

/// 关闭一条通道：登记关闭标记并丢弃写端（读线程下一轮读超时后退出）。
pub(crate) fn close_channel(key: u64) {
    mark_closed(key);
    WRITERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .remove(&key);
}

/// 生成一次性握手 key。
fn gen_key() -> String {
    let seq = KEY_SEQ.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{nanos}", std::process::id(), seq)
}

/// 父进程侧：为一个 worker 绑定 IPC 监听器，返回（端口, 握手 key）。
///
/// 绑定失败（端口耗尽/受限环境）返回 `None`——该 worker 无 IPC 通道，
/// 与 Node 中 `silent`/`stdio` 未含 `ipc` 的子进程形态一致（`process.send`
/// 不存在）。
pub(crate) fn spawn_worker_listener(worker_id: u64) -> Option<(u16, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    listener.set_nonblocking(true).ok()?;
    let key = gen_key();
    let owner = std::thread::current().id();
    let expect = key.clone();
    std::thread::Builder::new()
        .name(format!("aluka-ipc-accept-{worker_id}"))
        .spawn(move || accept_loop(listener, worker_id, &expect, owner))
        .ok()?;
    Some((port, key))
}

/// accept 线程：等待子进程连接并校验握手帧，随后转入读行循环。
fn accept_loop(listener: TcpListener, worker_id: u64, key: &str, owner: ThreadId) {
    loop {
        if is_closed(worker_id) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if stream.set_nodelay(true).is_err() {
                    return;
                }
                let _ = stream.set_read_timeout(Some(READ_POLL));
                let Ok(writer) = stream.try_clone() else {
                    return;
                };
                // 握手帧与后续帧必须共用同一个 BufReader：`read_line` 会预读
                // 到缓冲区，换 reader 会丢掉已预读的后续帧。
                let mut reader = BufReader::new(stream);
                if !read_handshake(&mut reader, key) {
                    // 握手不符：拒绝该连接并继续等待合法连接。
                    continue;
                }
                serve(reader, writer, worker_id, owner);
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(_) => return,
        }
    }
}

/// 读一行并校验握手帧（`{"hello":"<key>"}`）。
fn read_handshake(reader: &mut BufReader<TcpStream>, key: &str) -> bool {
    let expect = format!("{{\"hello\":\"{key}\"}}");
    let mut line = String::new();
    match reader.read_line(&mut line) {
        // EOF：未读到握手帧
        Ok(0) => false,
        Ok(_) => line.trim() == expect,
        // 非阻塞 / 超时读取：视为未握手
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            false
        }
        Err(_) => false,
    }
}

/// 父进程侧连接就绪：登记写端并持续读行投递 inbox。
fn serve(reader: BufReader<TcpStream>, writer: TcpStream, worker_id: u64, owner: ThreadId) {
    register_writer(worker_id, writer);
    let mut reader = reader;
    read_lines(&mut reader, worker_id, owner);
    // 对端 EOF：登记关闭标记（后续 send 一律 false）并向属主线程投递关闭通知。
    close_channel(worker_id);
    push_incoming(owner, worker_id, Incoming::Closed);
}

/// 登记（或替换）某条通道的写端。
fn register_writer(worker_id: u64, writer: TcpStream) {
    WRITERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(worker_id, Arc::new(Mutex::new(writer)));
}

/// 读行循环：一行投递一条 `Line`，EOF/错误投递 `Closed` 并结束。
fn read_lines(reader: &mut BufReader<TcpStream>, key: u64, owner: ThreadId) {
    let mut line = String::new();
    loop {
        if is_closed(key) {
            return;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                let text = line.trim_end_matches(['\r', '\n']).to_owned();
                if !text.is_empty() {
                    push_incoming(owner, key, Incoming::Line(text));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => return,
        }
    }
}

/// 父进程 → worker：写一行（通道不存在/已关闭返回 false）。
pub(crate) fn send_to_worker(worker_id: u64, line: &str) -> bool {
    if is_closed(worker_id) {
        return false;
    }
    let writer = WRITERS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&worker_id).cloned());
    let Some(writer) = writer else {
        return false;
    };
    let Ok(mut stream) = writer.lock() else {
        return false;
    };
    if stream.write_all(line.as_bytes()).is_err() || stream.flush().is_err() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// 子进程侧
// ---------------------------------------------------------------------------

/// 子进程侧通道状态（VM 线程独占）。
struct ChildIpc {
    /// 与父进程连接的写端
    writer: Mutex<TcpStream>,
    /// 通道是否仍连通（Node `process.connected`）
    connected: bool,
}

thread_local! {
    static CHILD_IPC: std::cell::RefCell<Option<ChildIpc>> =
        const { std::cell::RefCell::new(None) };
}

/// 子进程侧建立 IPC 连接（幂等）；无 IPC 环境变量/连接失败返回 false。
pub(crate) fn child_connect() -> bool {
    if CHILD_IPC.with(|c| c.borrow().is_some()) {
        return child_is_connected();
    }
    let Some(port) = std::env::var(ENV_PORT)
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        return false;
    };
    let key = std::env::var(ENV_KEY).unwrap_or_default();
    let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_nodelay(true);
    let Ok(mut writer) = stream.try_clone() else {
        return false;
    };
    // 握手帧先行（父进程 accept 线程校验后才登记写端）。
    let hello = format!("{{\"hello\":\"{key}\"}}\n");
    if writer.write_all(hello.as_bytes()).is_err() || writer.flush().is_err() {
        return false;
    }
    let _ = stream.set_read_timeout(Some(READ_POLL));
    // 读线程：属主 = 当前 VM 线程（堆句柄与事件派发都归它）。
    let owner = std::thread::current().id();
    let _ = std::thread::Builder::new()
        .name("aluka-ipc-child-read".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            read_lines(&mut reader, CHILD_CHANNEL_KEY, owner);
        });
    CHILD_IPC.with(|c| {
        *c.borrow_mut() = Some(ChildIpc {
            writer: Mutex::new(writer),
            connected: true,
        });
    });
    true
}

/// 子进程 → 父进程：写一行（未连接/写失败返回 false 并翻转 connected）。
pub(crate) fn child_send_line(line: &str) -> bool {
    CHILD_IPC.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if !state.connected {
            return false;
        }
        match state.writer.lock() {
            Ok(mut stream) => {
                if stream.write_all(line.as_bytes()).is_err() || stream.flush().is_err() {
                    state.connected = false;
                    false
                } else {
                    true
                }
            }
            Err(_) => {
                state.connected = false;
                false
            }
        }
    })
}

/// 子进程侧通道是否连通。
pub(crate) fn child_is_connected() -> bool {
    CHILD_IPC.with(|c| c.borrow().as_ref().is_some_and(|s| s.connected))
}
