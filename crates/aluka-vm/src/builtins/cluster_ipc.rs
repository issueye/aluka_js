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
//!   解析为堆值（与 `proc_common` 的 proc 事件泵同一纪律）；
//! - **握手容错（20260911 修复）**：`read_handshake` 对「读超时」不再判定为
//!   非法握手，改为带总期限（`HANDSHAKE_TIMEOUT`）的重试——连接已建立但
//!   握手帧稍后才到是正常时序，原实现会把该连接丢弃（`drop(stream)` 发 RST），
//!   使对端第一次写即 `ECONNRESET`，整条 IPC 面静默失效（实测约 1.5% 复现率，
//!   修复后 250 次连跑零复现）；
//! - **关闭标记的生效时机**：读行循环只在「本轮无数据可读」（读超时）时才
//!   响应 `is_closed`，内核缓冲区中已到达的帧一律先读净——否则父进程侧抢先
//!   `close_channel` 会丢掉子进程退出前写出的最后一帧（listening）。
//! - **子进程侧关闭（M5.2）**：对端 EOF 或本侧 `process.disconnect()`
//!   （`child_close_channel`，发 FIN 半关闭写端）都会翻转 `connected` 并投递
//!   `Incoming::Closed`（key 0），worker 侧据此派发 `'disconnect'` 并解除事件源
//!   保活（对齐 Node child_process 的 channel 'close'）。
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

/// 已为该 worker 成功起过 IPC 监听线程的集合。
///
/// 退出转接处据此决定「是否等待通道 EOF」：只要监听线程起过，对端就**应该**
/// 连接（`connect` 早于子进程业务逻辑），故等待是安全且必要的；反之上游
/// `bind`/线程创建失败时不必付出等待代价。
static SPAWNED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// 该 worker 是否成功起过 IPC 监听线程。
pub(crate) fn listener_spawned(key: u64) -> bool {
    SPAWNED
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.contains(&key))
}

/// 阻塞等待通道 EOF（`is_closed` 置位），上限 `timeout`；返回是否已关闭。
///
/// 语义对齐 Node：child_process 的 ipc 通道在子进程退出时先产生 EOF
/// （`'disconnect'`），随后才上报进程退出（`'exit'`）。本运行时这两件事是
/// **两条独立通路**（本模块的读线程 vs. 子进程退出事件源），故在退出转接处
/// 以短等待把次序钉死；同时保证对端**退出前写出的帧**（listening 等）已被
/// 读线程投递——读线程在 `close_channel` 之后会从循环顶部退出，若抢先置位
/// 会丢掉内核缓冲区里尚未读出的帧（实测可复现的偶发丢帧）。
///
/// 轮询粒度 1ms（读线程的 `READ_POLL` 为 100ms，但 EOF/数据到达会让阻塞
/// 读立即返回，故实际等待通常 <1ms）。
pub(crate) fn wait_channel_eof(key: u64, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_closed(key) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
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
    SPAWNED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(worker_id);
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
                // 握手阶段用**长超时**：超时只会让 `read_handshake` 重试，
                // 而 100ms 的轮询粒度会在对端稍慢送出握手帧时反复触发
                // （Windows 上超时读还可能让套接字进入不佳状态）。
                let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
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
                // 转入读行循环：回到 100ms 轮询粒度（用于 `is_closed` 的响应性）。
                let _ = reader.get_ref().set_read_timeout(Some(READ_POLL));
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

/// 握手帧读取的总期限（跨多次 `READ_POLL` 超时的重试预算）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 读一行并校验握手帧（`{"hello":"<key>"}`）。
///
/// **读超时（`READ_POLL`）不等于非法握手**：accept 到 `connect` 返回之间对端
/// 未必已把握手帧送出（两次调度之间可能超过 100ms），且一行可能跨多次 `read`
/// 才到齐。原先「一次 `read_line` 超时即 `false`」会让 accept 线程把该连接
/// 丢弃（`continue` 时 `stream` 被 drop → 发出 RST），对端随后的第一次写即
/// `ECONNRESET`——实测表现为 worker 的 `online` 帧写失败、整条 IPC 面静默
/// 失效（约 1.5% 概率的偶发缺陷）。
///
/// 此处改为「带总期限的重试」：`line` 跨重试累积（`read_line` 追加写入），
/// 直到读满一行、对端 EOF 或超过 `HANDSHAKE_TIMEOUT`。
fn read_handshake(reader: &mut BufReader<TcpStream>, key: &str) -> bool {
    let expect = format!("{{\"hello\":\"{key}\"}}");
    let deadline = std::time::Instant::now() + HANDSHAKE_TIMEOUT;
    let mut line = String::new();
    loop {
        match reader.read_line(&mut line) {
            // EOF：未读到握手帧
            Ok(0) => return false,
            Ok(_) => return line.trim() == expect,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if std::time::Instant::now() >= deadline {
                    return false;
                }
            }
            Err(_) => return false,
        }
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
///
/// **关闭标记只在「本轮无数据可读」时才生效**（读超时分支）：内核缓冲区里
/// 已到达的帧一律先读净再退出——否则 `close_channel` 抢先置位时会丢掉对端
/// 退出前写出的最后一帧（listening），这是实测可复现的偶发丢帧根因。
fn read_lines(reader: &mut BufReader<TcpStream>, key: u64, owner: ThreadId) {
    let mut line = String::new();
    loop {
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
                if is_closed(key) {
                    return;
                }
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
            // 对端 EOF / 本地关闭（读线程退出）：翻转连通标记并向属主线程投递
            // 关闭通知——worker 侧据此派发 `'disconnect'` 并解除事件源保活
            // （对齐 Node child_process 的 channel 'close'）。
            child_mark_closed();
            push_incoming(owner, CHILD_CHANNEL_KEY, Incoming::Closed);
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

/// 子进程侧通道翻转「未连通」并登记本地关闭标记。
///
/// 两个触发源：① 对端 EOF（读线程退出）；② 本侧 `process.disconnect()`
/// （`child_close_channel`）。标记一旦置位，`child_send_line` 一律返回 false，
/// 读线程也会在下一轮读超时后退出。
fn child_mark_closed() {
    mark_closed(CHILD_CHANNEL_KEY);
    CHILD_IPC.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.connected = false;
        }
    });
}

/// `process.disconnect()`（worker 侧）：向对端发 FIN（半关闭写端，对端读到 EOF
/// 后按 Node 语义派发 `'disconnect'`）并本地登记未连通。已断开返回 false。
pub(crate) fn child_close_channel() -> bool {
    if !child_is_connected() {
        return false;
    }
    CHILD_IPC.with(|c| {
        if let Some(state) = c.borrow().as_ref() {
            if let Ok(stream) = state.writer.lock() {
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        }
    });
    child_mark_closed();
    true
}
