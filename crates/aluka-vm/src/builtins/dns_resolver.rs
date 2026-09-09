//! `node:dns` resolve 家族真实 DNS 报文查询底座（M3.4b，A+ 路线）。
//!
//! 架构（与 VM 同步事件泵同构，不引入 tokio/hickory-resolver runtime）：
//! - `hickory-proto` 仅承担 DNS 报文编解码（纯 Rust、关默认特性）；
//! - 查询在**惰性初始化的后台 std::thread 工作线程**中同步执行
//!   （UDP non-block 超时 + 截断回落 TCP 带帧），结果写入进程级结果表；
//! - VM 侧以 `activate_event_source("dns-resolve", dns_resolver_pump)` 保活，
//!   泵每轮扫描 token → 完成的查询就地转换为 VM Value 并经既有
//!   `enqueue_dns` 队列派发 callback / Promise 兑现（时序与旧单级路径一致）；
//! - 请求走 `crossbeam-channel`（M5.1 worker_threads 将复用同款 channel 基建）。
//!
//! 口径折衷（保证既有离线对拍稳定，同时给出真实递归能力）：
//! - `resolve4/6`：系统 resolver 优先（覆盖 hosts / mDNS / 内网），失败才走报文查询；
//! - 其余 rrtype：本地可解析名（`localhost` 等，系统 resolver 成功）保持旧确定性
//!   空形态；非本地名走真实报文查询；
//! - `getServers()` 初始为空数组（与既有对拍一致），`setServers()` 后生效；
//! - 默认服务器：unix `/etc/resolv.conf` → Windows PowerShell 枚举 → 公共 DNS 兜底。

use crate::builtins::dns_promises::{enqueue_dns, lookup_host_addrs};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use crossbeam_channel::{Receiver, Sender, unbounded};
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{DNSClass, Name, RData, RecordType};
use hickory_proto::serialize::binary::BinEncodable;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// --- 数据模型 ---------------------------------------------------------------

/// 单条解析结果（RR 数据，已按 Node 形态去除尾部根点）。
#[derive(Debug, Clone)]
pub(crate) struct AnswerRecord {
    /// 记录负载。
    pub data: DnsRecord,
}

/// DNS 记录负载的 Rust 侧表达。
#[derive(Debug, Clone)]
pub(crate) enum DnsRecord {
    /// A / AAAA 地址。
    Addr(String),
    /// CNAME / NS / PTR 之类的单名记录。
    Name(String),
    /// MX 记录。
    Mx {
        /// 邮件交换主机。
        exchange: String,
        /// 优先级。
        priority: u16,
    },
    /// SOA 记录。
    Soa {
        /// 主服务器名。
        nsname: String,
        /// 管理员邮箱（首点原始形态，与 Node 一致）。
        hostmaster: String,
        /// 序列号。
        serial: u32,
        /// 刷新间隔。
        refresh: i32,
        /// 重试间隔。
        retry: i32,
        /// 过期秒数。
        expire: i32,
        /// 最小 TTL。
        minttl: u32,
    },
    /// SRV 记录。
    Srv {
        /// 目标主机。
        name: String,
        /// 端口。
        port: u16,
        /// 优先级。
        priority: u16,
        /// 权重。
        weight: u16,
    },
    /// TXT 记录（一段完整记录的字串块列表）。
    Txt(Vec<String>),
    /// NAPTR 记录。
    Naptr {
        /// 标志。
        flags: String,
        /// 服务。
        service: String,
        /// 正则。
        regexp: String,
        /// 替换名。
        replacement: String,
        /// 顺序。
        order: u16,
        /// 偏好。
        preference: u16,
    },
    /// CAA 记录（Node 形态：critical + 以 tag 命名的值键）。
    Caa {
        /// issuer critical 位。
        critical: bool,
        /// 属性 tag（issue / issuewild / iodef …）。
        tag: String,
        /// 值。
        value: String,
    },
    /// TLSA 记录。
    Tlsa {
        /// 证书用途。
        usage: u8,
        /// 选择器。
        selector: u8,
        /// 匹配类型。
        matching_type: u8,
        /// 证书数据（hex）。
        certificate: String,
    },
}

/// DNS 错误（Node/c-ares 错误码；hostname/syscall 在派发侧由调用上下文回填）。
#[derive(Debug, Clone)]
pub(crate) struct DnsError {
    /// Node 错误码（ENOTFOUND / ENODATA / ESERVFAIL / ETIMEOUT / …）。
    pub code: &'static str,
}

type QueryOutcome = Result<Vec<AnswerRecord>, DnsError>;

/// 待派发查询（VM 线程本地，凭 token 与结果表对账）。
pub(crate) enum PendingResolve {
    /// callback 风格：(err, result)。
    Callback(Value),
    /// Promise 风格：fulfill(result) / reject(err)。
    Promise {
        /// Promise fulfill resolver。
        fulfill: Value,
        /// Promise reject resolver。
        reject: Value,
    },
}

/// Node 值的构建形态（决定 records → JS Value 的组装形状）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum RrShape {
    /// 地址数组（resolve4/6、resolve A/AAAA）。
    AddrList,
    /// 名单数组（resolveCname/Ns/Ptr、reverse）。
    NameList,
    /// MX 对象数组。
    MxList,
    /// SOA 单对象。
    SoaObject,
    /// SRV 对象数组。
    SrvList,
    /// TXT 二维字串数组。
    TxtList,
    /// NAPTR 对象数组。
    NaptrList,
    /// CAA 对象数组。
    CaaList,
    /// TLSA 对象数组。
    TlsaList,
}

// --- 进程级工作线程桥 --------------------------------------------------------

/// 发往工作线程的查询请求。
struct QueryRequest {
    token: u64,
    qname: String,
    rrtype: RecordType,
    servers: Vec<SocketAddr>,
}

/// 懒初始化的工作线程桥：请求 channel（结果表经 `DRIVER_RESULTS` 静态表共享）。
struct WorkerBridge {
    req_tx: Sender<QueryRequest>,
}

static BRIDGE: OnceLock<WorkerBridge> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// 泵/线程共享的结果表（进程生命周期常驻，`Box::leak` 换取 `'static`）。
static DRIVER_RESULTS: OnceLock<&'static Mutex<HashMap<u64, QueryOutcome>>> = OnceLock::new();

/// 取（或首次创建）工作线程桥。
fn bridge() -> &'static WorkerBridge {
    BRIDGE.get_or_init(|| {
        let (req_tx, req_rx): (Sender<QueryRequest>, Receiver<QueryRequest>) = unbounded();
        let results: &'static Mutex<HashMap<u64, QueryOutcome>> =
            Box::leak(Box::new(Mutex::new(HashMap::new())));
        DRIVER_RESULTS
            .set(results)
            .expect("dns results table initialized once");
        std::thread::Builder::new()
            .name("aluka-dns-worker".to_owned())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let outcome = run_query(&req);
                    if let Ok(mut map) = results.lock() {
                        map.insert(req.token, outcome);
                    }
                }
            })
            .expect("spawn dns worker");
        WorkerBridge { req_tx }
    })
}

/// 取回某 token 的已完成查询结果（未完成 → None）。
fn take_outcome(token: u64) -> Option<QueryOutcome> {
    DRIVER_RESULTS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|mut map| map.remove(&token))
}

// --- 服务器配置 --------------------------------------------------------------

thread_local! {
    /// 用户可见的 DNS 服务器列表（`setServers` 写入；`getServers` 原样读出）。
    static DNS_SERVERS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// `dns.getServers()` 读出（初始空数组，与既有对拍一致）。
pub(crate) fn get_servers() -> Vec<String> {
    DNS_SERVERS.with(|g| g.borrow().clone().unwrap_or_default())
}

/// `dns.setServers(servers)` 写入（字符串数组形态）。
pub(crate) fn set_servers(servers: Vec<String>) {
    DNS_SERVERS.with(|g| *g.borrow_mut() = Some(servers));
}

/// 解析服务器字符串（`ip` / `ip:port` / `[v6]:port`）为 SocketAddr（默认 53）。
fn parse_server(s: &str) -> Option<SocketAddr> {
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, 53));
    }
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    // "ip:port"（v4 带端口时 SocketAddr 已覆盖，此处兜底非常规形式）。
    if let Some((host, port)) = s.rsplit_once(':') {
        if let (Ok(ip), Ok(p)) = (host.parse::<IpAddr>(), port.parse::<u16>()) {
            return Some(SocketAddr::new(ip, p));
        }
    }
    None
}

/// 有效服务器列表：用户设定优先；否则系统探测；末位公共 DNS 兜底。
fn effective_servers() -> Vec<SocketAddr> {
    let user = get_servers();
    if !user.is_empty() {
        return user.iter().filter_map(|s| parse_server(s)).collect();
    }
    system_default_servers()
        .iter()
        .filter_map(|s| parse_server(s))
        .collect()
}

/// 系统 DNS 服务器探测：unix resolv.conf → Windows PowerShell → 公共 DNS 兜底。
fn system_default_servers() -> &'static Vec<String> {
    static SERVERS: OnceLock<Vec<String>> = OnceLock::new();
    SERVERS.get_or_init(|| {
        let mut found = Vec::new();
        // unix：`/etc/resolv.conf` 的 nameserver 行。
        if let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") {
            for line in content.lines() {
                let line = line.trim();
                if let Some(addr) = line.strip_prefix("nameserver") {
                    let addr = addr.trim();
                    if addr.parse::<IpAddr>().is_ok() {
                        found.push(addr.to_owned());
                    }
                }
            }
        }
        // Windows：PowerShell 枚举适配器 DNS（语言中立、无 FFI）。
        #[cfg(windows)]
        if found.is_empty() {
            if let Ok(out) = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Get-DnsClientServerAddress -AddressFamily IPv4 | Select-Object -ExpandProperty ServerAddresses",
                ])
                .output()
            {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let a = line.trim();
                    if a.parse::<IpAddr>().is_ok() {
                        found.push(a.to_owned());
                    }
                }
            }
        }
        // 公共 DNS 兜底（文档化口径）。
        if found.is_empty() {
            found.push("8.8.8.8".to_owned());
            found.push("1.1.1.1".to_owned());
        }
        found
    })
}

// --- rrtype 映射与反向名 ------------------------------------------------------

/// JS rrtype 串 → RecordType（覆盖 Node resolve 家族全集；未知 → None 走旧空形态）。
fn to_record_type(rrtype: &str) -> Option<RecordType> {
    Some(match rrtype {
        "A" => RecordType::A,
        "AAAA" => RecordType::AAAA,
        "ANY" => RecordType::ANY,
        "CAA" => RecordType::CAA,
        "CNAME" => RecordType::CNAME,
        "MX" => RecordType::MX,
        "NAPTR" => RecordType::NAPTR,
        "NS" => RecordType::NS,
        "PTR" => RecordType::PTR,
        "SOA" => RecordType::SOA,
        "SRV" => RecordType::SRV,
        "TLSA" => RecordType::TLSA,
        "TXT" => RecordType::TXT,
        _ => return None,
    })
}

/// IPv4 / IPv6 → 反向 PTR 查询名（`…in-addr.arpa` / `…ip6.arpa`）。
pub(crate) fn arpa_name(ip: &str) -> Option<String> {
    let addr = ip.parse::<IpAddr>().ok()?;
    Some(match addr {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            let mut parts: Vec<String> = Vec::with_capacity(32);
            for seg in v6.segments().iter().rev() {
                let hex = format!("{seg:04x}");
                for c in hex.chars().rev() {
                    parts.push(c.to_string());
                }
            }
            format!("{}.ip6.arpa", parts.join("."))
        }
    })
}

// --- 查询提交与派发（VM 线程） -------------------------------------------------

thread_local! {
    static PENDING: RefCell<HashMap<u64, PendingResolve>> = RefCell::new(HashMap::new());
    static PENDING_META: RefCell<HashMap<u64, (RrShape, String, &'static str)>> =
        RefCell::new(HashMap::new());
}

/// 提交一次真实 DNS 查询并登记待派发项；激活 `dns-resolve` 事件源。
///
/// - `hostname`：Node 侧主机名/地址（错误对象 hostname 回填与此一致）；
/// - `qname`：实际查询名（reverse 场景为 arpa 名，其余同 hostname）；
/// - `rrtype` 为 Node rrtype 串；`syscall` 为 Node 风格 syscall 名。
pub(crate) fn start_resolve(
    vm: &mut Vm,
    hostname: &str,
    qname: &str,
    rrtype: &str,
    syscall: &'static str,
    shape: RrShape,
    pending: PendingResolve,
) -> bool {
    let Some(rt) = to_record_type(rrtype) else {
        return false;
    };
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let req = QueryRequest {
        token,
        qname: qname.to_owned(),
        rrtype: rt,
        servers: effective_servers(),
    };
    if bridge().req_tx.send(req).is_err() {
        return false;
    }
    PENDING.with(|p| p.borrow_mut().insert(token, pending));
    PENDING_META.with(|p| {
        p.borrow_mut()
            .insert(token, (shape, hostname.to_owned(), syscall));
    });
    vm.activate_event_source("dns-resolve", dns_resolver_pump);
    true
}

/// 真实查询入口（callback 风格）：提交后台查询；未知 rrtype → false（走旧形态）。
///
/// 返回值 false 时调用方回落旧确定性路径。
pub(crate) fn start_callback_resolve(
    vm: &mut Vm,
    hostname: &str,
    rrtype: &str,
    syscall: &'static str,
    shape: RrShape,
    cb: Value,
) -> bool {
    start_resolve(
        vm,
        hostname,
        hostname,
        rrtype,
        syscall,
        shape,
        PendingResolve::Callback(cb),
    )
}

/// 真实反向 PTR 查询（callback 风格）：qname 为 `arpa_name(ip)`。
pub(crate) fn start_callback_reverse(vm: &mut Vm, ip: &str, qname: &str, cb: Value) -> bool {
    start_resolve(
        vm,
        ip,
        qname,
        "PTR",
        "getHostByAddr",
        RrShape::NameList,
        PendingResolve::Callback(cb),
    )
}

/// 真实查询入口（Promise 风格）。
pub(crate) fn start_promise_resolve(
    vm: &mut Vm,
    hostname: &str,
    rrtype: &str,
    syscall: &'static str,
    shape: RrShape,
    fulfill: Value,
    reject: Value,
) -> bool {
    start_resolve(
        vm,
        hostname,
        hostname,
        rrtype,
        syscall,
        shape,
        PendingResolve::Promise { fulfill, reject },
    )
}

/// 真实反向 PTR 查询（Promise 风格）。
pub(crate) fn start_promise_reverse(
    vm: &mut Vm,
    ip: &str,
    qname: &str,
    fulfill: Value,
    reject: Value,
) -> bool {
    start_resolve(
        vm,
        ip,
        qname,
        "PTR",
        "getHostByAddr",
        RrShape::NameList,
        PendingResolve::Promise { fulfill, reject },
    )
}

// --- 对外判据与元数据（dns.rs / dns_promises.rs 路由用） ----------------------

/// rrtype → (结果形态, Node syscall 名)。返回 None 的类型（ANY 等）不走报文查询。
pub(crate) fn query_meta(rrtype: &str) -> Option<(RrShape, &'static str)> {
    Some(match rrtype {
        "A" => (RrShape::AddrList, "queryA"),
        "AAAA" => (RrShape::AddrList, "queryAaaa"),
        "CAA" => (RrShape::CaaList, "queryCaa"),
        "CNAME" => (RrShape::NameList, "queryCname"),
        "MX" => (RrShape::MxList, "queryMx"),
        "NAPTR" => (RrShape::NaptrList, "queryNaptr"),
        "NS" => (RrShape::NameList, "queryNs"),
        "PTR" => (RrShape::NameList, "queryPtr"),
        "SOA" => (RrShape::SoaObject, "querySoa"),
        "SRV" => (RrShape::SrvList, "querySrv"),
        "TLSA" => (RrShape::TlsaList, "queryTlsa"),
        "TXT" => (RrShape::TxtList, "queryTxt"),
        _ => return None,
    })
}

/// 该主机名是否应走报文查询：非单标签（含点）域名一律走真实报文；
/// `localhost` / 主机名这类单标签由系统 resolver 覆盖（保持既有确定性路径）。
pub(crate) fn needs_wire(hostname: &str) -> bool {
    lookup_host_addrs(hostname).is_none() || hostname.contains('.')
}

/// 反向查询是否需要走报文：合法且非回环的 IP 才发 PTR（回环保持旧空形态，
/// 离线环境确定性优先）。
pub(crate) fn reverse_needs_wire(ip: &str) -> bool {
    ip.parse::<IpAddr>()
        .map(|a| !a.is_loopback())
        .unwrap_or(false)
}

/// 判断 rrtype 是否走系统路径（A/AAAA/ANY 保持系统解析，与 Node getaddrinfo
/// 语义一致；ANY 的混合记录形态现有系统路径即可表达）。
pub(crate) fn rrtype_uses_system(rrtype: &str) -> bool {
    matches!(rrtype, "A" | "AAAA" | "ANY")
}

/// `dns-resolve` 事件源泵：对账已完成 token → 转 VM Value → 派发；
/// 排空中且队列清空后注销事件源。
fn dns_resolver_pump(vm: &mut Vm) -> Result<bool, VmError> {
    // 先收集已完成 token（避免常驻借用与 enqueue 交互）。
    let finished: Vec<u64> = PENDING.with(|p| {
        p.borrow()
            .keys()
            .copied()
            .filter(|t| {
                // 僅探測是否完成（不取出，取出在下一段統一做）。
                DRIVER_RESULTS
                    .get()
                    .and_then(|m| m.lock().ok())
                    .is_some_and(|map| map.contains_key(t))
            })
            .collect()
    });
    let mut progressed = false;
    for token in finished {
        let Some(outcome) = take_outcome(token) else {
            continue;
        };
        let pending = PENDING.with(|p| p.borrow_mut().remove(&token));
        let meta = PENDING_META.with(|p| p.borrow_mut().remove(&token));
        let (Some(pending), Some((shape, hostname, syscall))) = (pending, meta) else {
            continue;
        };
        dispatch(vm, pending, shape, &hostname, syscall, outcome)?;
        progressed = true;
    }
    let empty = PENDING.with(|p| p.borrow().is_empty());
    if empty && !progressed {
        vm.deactivate_event_source("dns-resolve");
    }
    Ok(progressed)
}

/// 完成项统一派发：callback `(err, result)` / Promise fulfill / reject。
fn dispatch(
    vm: &mut Vm,
    pending: PendingResolve,
    shape: RrShape,
    hostname: &str,
    syscall: &'static str,
    outcome: QueryOutcome,
) -> Result<(), VmError> {
    match (pending, outcome) {
        (PendingResolve::Callback(cb), Ok(records)) => {
            let value = records_value(vm, records, shape);
            enqueue_dns(vm, cb, vec![Value::Null, value]);
        }
        (PendingResolve::Callback(cb), Err(err)) => {
            let e = dns_query_error(vm, err.code, hostname, syscall);
            enqueue_dns(vm, cb, vec![e, Value::Null]);
        }
        (PendingResolve::Promise { fulfill, .. }, Ok(records)) => {
            let value = records_value(vm, records, shape);
            enqueue_dns(vm, fulfill, vec![value]);
        }
        (PendingResolve::Promise { reject, .. }, Err(err)) => {
            let e = dns_query_error(vm, err.code, hostname, syscall);
            enqueue_dns(vm, reject, vec![e]);
        }
    }
    Ok(())
}

/// Node 风格 DNS 错误对象：`{syscall} {code} {hostname}`，
/// 附 code / errno / hostname / syscall 属性。
fn dns_query_error(vm: &mut Vm, code: &str, hostname: &str, syscall: &str) -> Value {
    let err = vm.alloc_error_instance(&format!("{syscall} {code} {hostname}"));
    let scode = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "code", Value::Object(scode));
    let serrno = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "errno", Value::Object(serrno));
    let shost = vm.alloc_string(hostname.to_owned());
    let _ = vm.set_property(Value::Object(err), "hostname", Value::Object(shost));
    let ssys = vm.alloc_string(syscall.to_owned());
    let _ = vm.set_property(Value::Object(err), "syscall", Value::Object(ssys));
    Value::Object(err)
}

// --- records → VM Value -----------------------------------------------------

fn strip_dot(name: &Name) -> String {
    name.to_utf8().trim_end_matches('.').to_owned()
}

/// 按形态把记录列表组装为 JS Value。
fn records_value(vm: &mut Vm, records: Vec<AnswerRecord>, shape: RrShape) -> Value {
    match shape {
        RrShape::AddrList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Addr(a) => Some(Value::Object(vm.alloc_string(a))),
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::NameList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Name(n) => Some(Value::Object(vm.alloc_string(n))),
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::MxList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Mx { exchange, priority } => {
                        let o = vm.alloc_ordinary();
                        let se = vm.alloc_string(exchange);
                        let _ = vm.set_property(Value::Object(o), "exchange", Value::Object(se));
                        let _ = vm.set_property(
                            Value::Object(o),
                            "priority",
                            Value::Number(priority as f64),
                        );
                        Some(Value::Object(o))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::SoaObject => {
            let o = vm.alloc_ordinary();
            if let Some(AnswerRecord {
                data:
                    DnsRecord::Soa {
                        nsname,
                        hostmaster,
                        serial,
                        refresh,
                        retry,
                        expire,
                        minttl,
                    },
                ..
            }) = records.into_iter().next()
            {
                let pairs: Vec<(&str, Value)> = vec![
                    ("nsname", {
                        let s = vm.alloc_string(nsname);
                        Value::Object(s)
                    }),
                    ("hostmaster", {
                        let s = vm.alloc_string(hostmaster);
                        Value::Object(s)
                    }),
                    ("serial", Value::Number(serial as f64)),
                    ("refresh", Value::Number(refresh as f64)),
                    ("retry", Value::Number(retry as f64)),
                    ("expire", Value::Number(expire as f64)),
                    ("minttl", Value::Number(minttl as f64)),
                ];
                for (k, v) in pairs {
                    let _ = vm.set_property(Value::Object(o), k, v);
                }
            }
            Value::Object(o)
        }
        RrShape::SrvList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Srv {
                        name,
                        port,
                        priority,
                        weight,
                    } => {
                        let o = vm.alloc_ordinary();
                        let sn = vm.alloc_string(name);
                        let _ = vm.set_property(Value::Object(o), "name", Value::Object(sn));
                        let _ =
                            vm.set_property(Value::Object(o), "port", Value::Number(port as f64));
                        let _ = vm.set_property(
                            Value::Object(o),
                            "priority",
                            Value::Number(priority as f64),
                        );
                        let _ = vm.set_property(
                            Value::Object(o),
                            "weight",
                            Value::Number(weight as f64),
                        );
                        Some(Value::Object(o))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::TxtList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Txt(chunks) => {
                        let inner: Vec<Value> = chunks
                            .into_iter()
                            .map(|c| Value::Object(vm.alloc_string(c)))
                            .collect();
                        Some(Value::Object(vm.alloc_array(inner)))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::NaptrList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Naptr {
                        flags,
                        service,
                        regexp,
                        replacement,
                        order,
                        preference,
                    } => {
                        let o = vm.alloc_ordinary();
                        for (k, v) in [
                            ("flags", flags),
                            ("service", service),
                            ("regexp", regexp),
                            ("replacement", replacement),
                        ] {
                            let s = vm.alloc_string(v);
                            let _ = vm.set_property(Value::Object(o), k, Value::Object(s));
                        }
                        let _ =
                            vm.set_property(Value::Object(o), "order", Value::Number(order as f64));
                        let _ = vm.set_property(
                            Value::Object(o),
                            "preference",
                            Value::Number(preference as f64),
                        );
                        Some(Value::Object(o))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::CaaList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Caa {
                        critical,
                        tag,
                        value,
                    } => {
                        let o = vm.alloc_ordinary();
                        let _ = vm.set_property(
                            Value::Object(o),
                            "critical",
                            Value::Number(if critical { 128.0 } else { 0.0 }),
                        );
                        let sv = vm.alloc_string(value);
                        let _ = vm.set_property(Value::Object(o), &tag, Value::Object(sv));
                        Some(Value::Object(o))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
        RrShape::TlsaList => {
            let vals: Vec<Value> = records
                .into_iter()
                .filter_map(|r| match r.data {
                    DnsRecord::Tlsa {
                        usage,
                        selector,
                        matching_type,
                        certificate,
                    } => {
                        let o = vm.alloc_ordinary();
                        let _ =
                            vm.set_property(Value::Object(o), "usage", Value::Number(usage as f64));
                        let _ = vm.set_property(
                            Value::Object(o),
                            "selector",
                            Value::Number(selector as f64),
                        );
                        let _ = vm.set_property(
                            Value::Object(o),
                            "matchingType",
                            Value::Number(matching_type as f64),
                        );
                        let sc = vm.alloc_string(certificate);
                        let _ = vm.set_property(Value::Object(o), "certificate", Value::Object(sc));
                        Some(Value::Object(o))
                    }
                    _ => None,
                })
                .collect();
            Value::Object(vm.alloc_array(vals))
        }
    }
}

// --- 工作线程：报文构造与收发 --------------------------------------------------

/// 单条查询的端到端执行（工作线程内同步阻塞）。
fn run_query(req: &QueryRequest) -> QueryOutcome {
    let err = |code: &'static str| DnsError { code };
    if req.servers.is_empty() {
        return Err(err("ECONNREFUSED"));
    }
    // 构造查询报文。
    let name = Name::from_ascii(req.qname.clone()).map_err(|_| err("EBADNAME"))?;
    let id = (req.token & 0xFFFF) as u16;
    let mut msg = Message::new();
    msg.set_id(id);
    msg.set_message_type(MessageType::Query);
    msg.set_op_code(OpCode::Query);
    msg.set_recursion_desired(true);
    let mut q = Query::query(name, req.rrtype);
    q.set_query_class(DNSClass::IN);
    msg.add_query(q);
    let wire = msg.to_bytes().map_err(|_| err("EFORMERR"))?;

    // 按服务器顺序尝试（每台 2 次机会：UDP → 截断则 TCP）。
    let mut last = err("ETIMEOUT");
    for server in &req.servers {
        match exchange_udp(&wire, *server, id) {
            Ok(resp) => {
                if resp.truncated() {
                    match exchange_tcp(&wire, *server, id) {
                        Ok(tresp) => return interpret(&tresp, req.rrtype),
                        Err(e) => last = e,
                    }
                } else {
                    return interpret(&resp, req.rrtype);
                }
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// UDP 收发（2.5s 读超时；响应 id 校验）。
fn exchange_udp(wire: &[u8], server: SocketAddr, id: u16) -> Result<Message, DnsError> {
    let bind: SocketAddr = if server.is_ipv4() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    };
    let mk = |code: &'static str| DnsError { code };
    let sock = std::net::UdpSocket::bind(bind).map_err(|_| mk("ECONNREFUSED"))?;
    sock.set_read_timeout(Some(Duration::from_millis(2500)))
        .map_err(|_| mk("ETIMEOUT"))?;
    sock.send_to(wire, server).map_err(|_| mk("ECONNREFUSED"))?;
    let mut buf = vec![0u8; 4096];
    let (n, _src) = sock.recv_from(&mut buf).map_err(|_| mk("ETIMEOUT"))?;
    let resp = Message::from_vec(&buf[..n]).map_err(|_| mk("EBADRESP"))?;
    if resp.id() != id {
        return Err(mk("EBADRESP"));
    }
    Ok(resp)
}

/// TCP 收发（截断回落；DNS-over-TCP 2 字节大端长度帧）。
fn exchange_tcp(wire: &[u8], server: SocketAddr, id: u16) -> Result<Message, DnsError> {
    let mk = |code: &'static str| DnsError { code };
    let mut sock = std::net::TcpStream::connect_timeout(&server, Duration::from_millis(2500))
        .map_err(|_| mk("ECONNREFUSED"))?;
    sock.set_read_timeout(Some(Duration::from_millis(2500)))
        .map_err(|_| mk("ETIMEOUT"))?;
    sock.set_write_timeout(Some(Duration::from_millis(2500)))
        .map_err(|_| mk("ETIMEOUT"))?;
    let len = (wire.len() as u16).to_be_bytes();
    let mut frame = Vec::with_capacity(wire.len() + 2);
    frame.extend_from_slice(&len);
    frame.extend_from_slice(wire);
    sock.write_all(&frame).map_err(|_| mk("ECONNREFUSED"))?;
    let mut h2 = [0u8; 2];
    sock.read_exact(&mut h2).map_err(|_| mk("EOF"))?;
    let n = u16::from_be_bytes(h2) as usize;
    let mut buf = vec![0u8; n];
    sock.read_exact(&mut buf).map_err(|_| mk("EOF"))?;
    let resp = Message::from_vec(&buf).map_err(|_| mk("EBADRESP"))?;
    if resp.id() != id {
        return Err(mk("EBADRESP"));
    }
    Ok(resp)
}

/// 响应解释：rcode → Node 错误码；NoError 时映射答案（空 → 空结果）。
fn interpret(resp: &Message, qtype: RecordType) -> QueryOutcome {
    let err = |code: &'static str| DnsError { code };
    let records: Vec<AnswerRecord> = resp
        .answers()
        .iter()
        .filter(|rec| qtype == RecordType::ANY || rec.record_type() == qtype)
        .filter_map(|rec| map_rdata(rec.data()).map(|data| AnswerRecord { data }))
        .collect();
    match resp.response_code() {
        // NoError：无论有无答案都成功（Node/c-ares 对无记录域返回空数组而非
        // 错误；NXDomain 等才报错）。
        ResponseCode::NoError => Ok(records),
        ResponseCode::NXDomain => Err(err("ENOTFOUND")),
        ResponseCode::ServFail => Err(err("ESERVFAIL")),
        ResponseCode::Refused => Err(err("EREFUSED")),
        ResponseCode::NotImp => Err(err("ENOTIMP")),
        ResponseCode::FormErr => Err(err("EFORMERR")),
        _ => Err(err("ESERVFAIL")),
    }
}

/// RData → DnsRecord（未覆盖类型 → None）。
fn map_rdata(data: &RData) -> Option<DnsRecord> {
    Some(match data {
        RData::A(a) => DnsRecord::Addr(a.0.to_string()),
        RData::AAAA(a) => DnsRecord::Addr(a.0.to_string()),
        RData::CNAME(n) => DnsRecord::Name(strip_dot(&n.0)),
        RData::NS(n) => DnsRecord::Name(strip_dot(&n.0)),
        RData::PTR(n) => DnsRecord::Name(strip_dot(&n.0)),
        RData::MX(mx) => DnsRecord::Mx {
            exchange: strip_dot(mx.exchange()),
            priority: mx.preference(),
        },
        RData::SOA(soa) => DnsRecord::Soa {
            nsname: strip_dot(soa.mname()),
            hostmaster: strip_dot(soa.rname()),
            serial: soa.serial(),
            refresh: soa.refresh(),
            retry: soa.retry(),
            expire: soa.expire(),
            minttl: soa.minimum(),
        },
        RData::SRV(srv) => DnsRecord::Srv {
            name: strip_dot(srv.target()),
            port: srv.port(),
            priority: srv.priority(),
            weight: srv.weight(),
        },
        RData::TXT(txt) => DnsRecord::Txt(
            txt.txt_data()
                .iter()
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect(),
        ),
        RData::NAPTR(nap) => DnsRecord::Naptr {
            flags: String::from_utf8_lossy(nap.flags()).into_owned(),
            service: String::from_utf8_lossy(nap.services()).into_owned(),
            regexp: String::from_utf8_lossy(nap.regexp()).into_owned(),
            replacement: strip_dot(nap.replacement()),
            order: nap.order(),
            preference: nap.preference(),
        },
        RData::CAA(caa) => DnsRecord::Caa {
            critical: caa.issuer_critical(),
            tag: caa.tag().to_string(),
            // raw_value 保留原始字节；与 Node 返回字符串值一致（Value 类型
            // 面在 0.25 已拆分 issue/iodef，raw 是跨类型通用取法）。
            value: String::from_utf8_lossy(caa.raw_value()).into_owned(),
        },
        RData::TLSA(tlsa) => DnsRecord::Tlsa {
            usage: u8::from(tlsa.cert_usage()),
            selector: u8::from(tlsa.selector()),
            matching_type: u8::from(tlsa.matching()),
            certificate: tlsa
                .cert_data()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        },
        _ => return None,
    })
}

// --- VM 便捷入口（callback / Promise 两家族共用；定义见「查询提交与派发」节） --

// (start_callback_resolve / start_callback_reverse / start_promise_resolve /
//  start_promise_reverse 均已在上方提供，调用方直接使用即可。)

// --- 单测 --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arpa_v4_and_v6() {
        assert_eq!(arpa_name("127.0.0.1").unwrap(), "1.0.0.127.in-addr.arpa");
        assert_eq!(
            arpa_name("::1").unwrap(),
            "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa"
        );
        assert!(arpa_name("not-an-ip").is_none());
    }

    #[test]
    fn rrtype_covers_node_set() {
        for t in [
            "A", "AAAA", "ANY", "CAA", "CNAME", "MX", "NAPTR", "NS", "PTR", "SOA", "SRV", "TLSA",
            "TXT",
        ] {
            assert!(to_record_type(t).is_some(), "{t}");
        }
        assert!(to_record_type("BOGUS").is_none());
    }

    #[test]
    fn server_parse_forms() {
        let sa = parse_server("1.1.1.1").unwrap();
        assert_eq!(sa.port(), 53);
        let sa2 = parse_server("127.0.0.1:53535").unwrap();
        assert_eq!(sa2.port(), 53535);
        assert!(parse_server("not-a-server").is_none());
    }

    #[test]
    fn query_wire_roundtrip_no_network() {
        // 纯编解码自检（不触网）：构造查询报文并读回 id/qname/rrtype。
        let name = Name::from_ascii("example.test").unwrap();
        let mut msg = Message::new();
        msg.set_id(42);
        msg.set_message_type(MessageType::Query);
        msg.set_op_code(OpCode::Query);
        msg.set_recursion_desired(true);
        let mut q = Query::query(name.clone(), RecordType::MX);
        q.set_query_class(DNSClass::IN);
        msg.add_query(q);
        let wire = msg.to_bytes().unwrap();
        let back = Message::from_vec(&wire).unwrap();
        assert_eq!(back.id(), 42);
        assert_eq!(back.queries()[0].query_type(), RecordType::MX);
        assert_eq!(
            back.queries()[0].name().to_utf8().trim_end_matches('.'),
            "example.test"
        );
    }

    #[test]
    fn interpret_nxdomain_and_nodata() {
        // NXDomain → ENOTFOUND。
        let mut nx = Message::new();
        nx.set_id(1);
        nx.set_message_type(MessageType::Response);
        nx.set_response_code(ResponseCode::NXDomain);
        let out = interpret(&nx, RecordType::A);
        assert_eq!(out.unwrap_err().code, "ENOTFOUND");

        // NoError + 无答案 → 成功空数组（对齐 Node：无记录域不报错）。
        let mut empty = Message::new();
        empty.set_id(1);
        empty.set_message_type(MessageType::Response);
        empty.set_response_code(ResponseCode::NoError);
        let out2 = interpret(&empty, RecordType::MX);
        assert_eq!(out2.unwrap().len(), 0);
    }
}
