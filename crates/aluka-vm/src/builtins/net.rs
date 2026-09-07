//! `node:net` 内置模块（Phase 5）：TCP 服务器与客户端。
//!
//! 与 Node.js 22 LTS 标准（`nodenet/net.go`）逐字对齐的语义：
//! - `net.createServer([connectionListener])` → Server（`listen` / `close` /
//!   `address` / `getConnections` + `'connection'`/`'listening'`/`'close'`/`'error'` 事件）；
//! - `net.connect` / `net.createConnection`（对象与 `(port, host)` 两种签名）→
//!   Socket（`write`/`end`/`destroy`/`pipe` + `'connect'`/`'data'`/`'end'`/`'close'`/`'error'`）；
//! - `net.isIP` / `isIPv4` / `isIPv6`（Go `net.ParseIP` 语义：IPv4 映射地址归 4）；
//! - `net.BlockList`（addAddress/addSubnet/addRange/check + `rules` 快照数组）；
//! - `net.SocketAddress`（address/port/family/flowlabel，family 规范化为小写）。
//!
//! 真实 I/O：`std::net::TcpListener` / `TcpStream`（非阻塞）。事件经
//! [`NET_SHARED`] 的待派发队列由 `activate_event_source("net", net_pump)`
//! 泵函数在事件循环里派发，对齐 Go 的 goroutine + PostTask 观察时序：
//! listen/connect 回调先于同名事件、连接回调先于对端 `'connection'`、
//! write 回调异步、生命周期事件 `'end'`→`'close'` 成对异步派发。
//! `data` 事件携带 Buffer 实例（Node 语义）。
//! 全部实体终结且队列排空后自动注销事件源，进程正常退出。

use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::Mutex;

/// 单次读取上限（对齐 Go 读循环 4096 字节缓冲）。
const READ_CHUNK: usize = 4096;

/// 事件监听器条目（回调 + once 标记）。
struct NetListener {
    /// 监听器回调。
    cb: Value,
    /// 是否单次触发。
    once: bool,
}

/// 待派发动作：泵在事件循环线程内解释执行的最小单元。
enum NetAction {
    /// 组合动作（按顺序执行，如「连接回调 → 'connect' 事件」序列）。
    Composite(Vec<NetAction>),
    /// 触发目标实例的事件（无监听器的 `'error'` 按未捕获异常上抛）。
    Emit {
        /// 目标实例（server / socket 对象值）。
        target: Value,
        /// 事件名。
        event: String,
        /// 事件实参。
        args: Vec<Value>,
    },
    /// 以 `this` 调用回调。
    Call {
        /// 回调值。
        cb: Value,
        /// this 绑定。
        this: Value,
        /// 调用实参。
        args: Vec<Value>,
    },
    /// 触发已终结实例的事件（监听器在实例移除前快照，如 end/close/error）。
    EmitWith {
        /// 目标实例对象值。
        target: Value,
        /// 事件名。
        event: String,
        /// 事件实参。
        args: Vec<Value>,
        /// 预快照的监听器回调。
        listeners: Vec<Value>,
    },
}

/// Socket 实例共享状态（键 = 实例 `ObjectRef` 句柄值）。
struct SocketState {
    /// 实例对象值（派发事件时的 this）。
    obj: Value,
    /// 已连接的 TCP 流（非阻塞；None = 未连接）。
    stream: Option<TcpStream>,
    /// 待建立的连接（host, port），由泵完成真实 `connect`。
    connecting: Option<(String, u16)>,
    /// 生命周期事件（end/close）是否已入队（幂等守卫）。
    closed: bool,
    /// `net.connect(options, connectListener)` / 位置签名里的连接监听器。
    connect_listener: Option<Value>,
    /// 事件监听器表。
    listeners: HashMap<String, Vec<NetListener>>,
    /// pipe 目标（`socket.pipe(dest)` 的数据转发对象）。
    pipe_dest: Option<Value>,
}

/// Server 实例共享状态。
struct ServerState {
    /// 实例对象值。
    obj: Value,
    /// 监听中的 TCP 监听器（非阻塞；close 后置 None，bound_addr 保留供 address()）。
    listener: Option<TcpListener>,
    /// close 幂等守卫。
    closed: bool,
    /// 绑定地址缓存（close 后 `address()` 仍返回对象，对齐 Go 观测）。
    bound_addr: Option<SocketAddr>,
    /// 事件监听器表。
    listeners: HashMap<String, Vec<NetListener>>,
    /// `createServer(connectionListener)` 的连接监听器。
    conn_listener: Option<Value>,
}

/// net 模块全部共享状态（事件源泵与之交互的唯一通道）。
#[derive(Default)]
struct NetShared {
    /// 存活的 Server（按创建序，保证泵轮询确定性）。
    servers: Vec<(u32, ServerState)>,
    /// 存活的 Socket（按创建序）。
    sockets: Vec<(u32, SocketState)>,
    /// 待派发动作队列（FIFO）。
    pending: VecDeque<NetAction>,
}

static NET_SHARED: Mutex<Option<NetShared>> = Mutex::new(None);

/// BlockList 实例规则表（实例句柄 → 规则列表）。
static BLOCKLISTS: Mutex<Option<HashMap<u32, Vec<BlockRule>>>> = Mutex::new(None);

/// 黑名单条目（Range / Subnet / Address，与 Go blockListState 对应）。
#[derive(Debug, Clone)]
enum BlockRule {
    /// 地址区间（仅 IPv4 参与匹配，对齐 Go ipRangeContains）。
    Range(u32, u32),
    /// 子网（已掩码网络地址 + 前缀长度）。
    Subnet(u32, u32),
    /// 精确地址（存原始串，check 按规范化形式比对，对齐 Go）。
    Address(String),
}

/// 在互斥锁内访问 net 共享状态（闭包内禁止触碰 `Vm` 与其他锁）。
fn with_net<R>(f: impl FnOnce(&mut NetShared) -> R) -> R {
    let mut guard = NET_SHARED.lock().unwrap();
    f(guard.get_or_insert_with(NetShared::default))
}

/// 在互斥锁内访问指定实例（socket 或 server）的监听器表。
fn with_listeners<R>(
    id: u32,
    f: impl FnOnce(&mut HashMap<String, Vec<NetListener>>) -> R,
) -> Option<R> {
    with_net(|n| {
        if let Some((_, state)) = n.sockets.iter_mut().find(|(sid, _)| *sid == id) {
            return Some(f(&mut state.listeners));
        }
        n.servers
            .iter_mut()
            .find(|(sid, _)| *sid == id)
            .map(|(_, state)| f(&mut state.listeners))
    })
}

/// 在互斥锁内访问指定 BlockList 实例的规则表。
fn with_blocklist<R>(id: u32, f: impl FnOnce(&mut Vec<BlockRule>) -> R) -> Option<R> {
    let mut guard = BLOCKLISTS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let rules = map.entry(id).or_default();
    Some(f(rules))
}

/// `require("net")` / `require("node:net")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef { name: "net", build };

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in [
        "createServer",
        "connect",
        "createConnection",
        "isIP",
        "isIPv4",
        "isIPv6",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("net.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    // 构造器：`new net.BlockList()` 经 do_construct 的注册表分支进入处理器。
    for ctor in ["BlockList", "SocketAddress"] {
        let fn_ref = vm.alloc_native_fn(&format!("net.{ctor}"));
        set_module_prop(vm, obj, ctor, Value::Object(fn_ref))?;
    }

    register_handler(registry, "net", "createServer", net_create_server);
    register_handler(registry, "net", "connect", net_connect);
    register_handler(registry, "net", "createConnection", net_connect);
    register_handler(registry, "net", "isIP", net_is_ip);
    register_handler(registry, "net", "isIPv4", net_is_ipv4);
    register_handler(registry, "net", "isIPv6", net_is_ipv6);
    register_handler(registry, "net", "BlockList", net_blocklist_ctor);
    register_handler(registry, "net", "SocketAddress", net_socket_address_ctor);

    // 实例方法命名空间：socket / server 共用一套事件与工具方法。
    let table: &[(&str, BuiltinHandler)] = &[
        ("on", net_on),
        ("addListener", net_on),
        ("once", net_once),
        ("emit", net_emit_handler),
        ("off", net_off),
        ("removeListener", net_off),
        ("listenerCount", net_listener_count),
        ("write", net_socket_write),
        ("end", net_socket_end),
        ("destroy", net_socket_destroy),
        ("address", net_socket_address),
        ("pipe", net_socket_pipe),
        ("listen", net_server_listen),
        ("close", net_server_close),
        ("getConnections", net_server_get_connections),
        ("setEncoding", net_noop_self),
        ("setNoDelay", net_noop_self),
        ("setTimeout", net_noop_self),
        ("setKeepAlive", net_noop_self),
        ("ref", net_noop_self),
        ("unref", net_noop_self),
        ("pause", net_noop_self),
        ("resume", net_noop_self),
    ];
    for ns in ["net:socket", "net:server"] {
        for (method, handler) in table {
            register_handler(registry, ns, method, *handler);
        }
    }
    // server 的 address 返回监听地址（含 family），socket 返回本地地址。
    register_handler(registry, "net:server", "address", net_server_address);
    for (method, handler) in [
        ("addAddress", blocklist_add_address as BuiltinHandler),
        ("addSubnet", blocklist_add_subnet),
        ("addRange", blocklist_add_range),
        ("check", blocklist_check),
    ] {
        register_handler(registry, "net:blocklist", method, handler);
    }

    Ok(obj)
}

/// 判断值是否为可调用对象（闭包 / 原生函数 / 原生构造器）。
fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
    ))
}

/// 判断对象值是否为普通对象（Ordinary 堆对象，用于 options 解析）。
fn is_plain_object(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Ordinary { .. })
    ))
}

/// 创建 socket 实例对象（挂 `_builtinNs` 与全部方法属性）。
fn create_socket_instance(vm: &mut Vm) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("net:socket".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in [
        "write",
        "end",
        "destroy",
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "listenerCount",
        "address",
        "pipe",
        "setEncoding",
        "setNoDelay",
        "setTimeout",
        "setKeepAlive",
        "ref",
        "unref",
        "pause",
        "resume",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("net:socket.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    let _ = vm.set_property(Value::Object(obj), "bytesRead", Value::Number(0.0));
    obj
}

/// 创建 server 实例对象。
fn create_server_instance(vm: &mut Vm) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("net:server".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in [
        "listen",
        "close",
        "address",
        "getConnections",
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "listenerCount",
        "ref",
        "unref",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("net:server.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    let _ = vm.set_property(Value::Object(obj), "listening", Value::Boolean(false));
    let _ = vm.set_property(Value::Object(obj), "maxConnections", Value::Number(0.0));
    obj
}

/// 把 socket 的本端 / 远端地址写入实例属性（对齐 Go setAddrProps）。
fn set_addr_props(vm: &mut Vm, obj: ObjectRef, stream: &TcpStream) {
    let (local, peer) = (stream.local_addr().ok(), stream.peer_addr().ok());
    if let Some(addr) = local {
        let s_alloc0 = vm.alloc_string(addr.ip().to_string());
        let _ = vm.set_property(Value::Object(obj), "localAddress", Value::Object(s_alloc0));
        let _ = vm.set_property(
            Value::Object(obj),
            "localPort",
            Value::Number(addr.port() as f64),
        );
    }
    if let Some(addr) = peer {
        let s_alloc0 = vm.alloc_string(addr.ip().to_string());
        let _ = vm.set_property(Value::Object(obj), "remoteAddress", Value::Object(s_alloc0));
        let _ = vm.set_property(
            Value::Object(obj),
            "remotePort",
            Value::Number(addr.port() as f64),
        );
        let s_alloc0 = vm.alloc_string("IPv4".to_owned());
        let _ = vm.set_property(Value::Object(obj), "remoteFamily", Value::Object(s_alloc0));
    }
}

// --- 模块级 API -----------------------------------------------------------

/// `net.createServer([connectionListener])`：创建 TCP 服务器实例。
fn net_create_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let listener = args.iter().rev().find(|a| is_function(vm, **a)).copied();
    let obj = create_server_instance(vm);
    with_net(|n| {
        n.servers.push((
            obj.0,
            ServerState {
                obj: Value::Object(obj),
                listener: None,
                bound_addr: None,
                closed: false,
                listeners: HashMap::new(),
                conn_listener: listener,
            },
        ));
    });
    Ok(Value::Object(obj))
}

/// `net.connect(options[, connectListener])` / `net.connect(port[, host][, cb])`。
///
/// 参数解析对齐 Go：函数（任意位置，取最后一个）→ 连接监听器；对象 →
/// 读 `host` / `port` 属性；数字 → port；字符串 → host。
fn net_connect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut host = "127.0.0.1".to_owned();
    let mut port = 0u16;
    let mut connect_listener: Option<Value> = None;
    for a in args {
        if is_function(vm, *a) {
            connect_listener = Some(*a);
        } else if is_plain_object(vm, *a) {
            if let Ok(v) = vm.get_property(*a, "host") {
                if !matches!(v, Value::Undefined | Value::Null) {
                    let s = vm.format_value(v);
                    if !s.is_empty() {
                        host = s;
                    }
                }
            }
            if let Ok(Value::Number(n)) = vm.get_property(*a, "port") {
                port = n as u16;
            }
        } else if let Value::Number(n) = a {
            port = *n as u16;
        } else if matches!(a, Value::Object(_)) {
            // 堆字符串参数视作 host（对齐 Go TypeString 分支）。
            host = vm.format_value(*a);
        }
    }

    let obj = create_socket_instance(vm);
    with_net(|n| {
        n.sockets.push((
            obj.0,
            SocketState {
                obj: Value::Object(obj),
                stream: None,
                connecting: Some((host, port)),
                closed: false,
                connect_listener,
                listeners: HashMap::new(),
                pipe_dest: None,
            },
        ));
    });
    // 连接在泵里完成（保持 connect 回调异步于同步代码块）。
    vm.activate_event_source("net", net_pump);
    Ok(Value::Object(obj))
}

/// `net.isIP(input)`：IPv4 → 4，IPv6 → 6，否则 0（IPv4 映射地址归 4，对齐 Go To4）。
fn net_is_ip(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(v) = args.first() else {
        return Ok(Value::Number(0.0));
    };
    let text = vm.format_value(*v);
    let family = match text.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => 4.0,
        Ok(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_some() => 4.0,
        Ok(IpAddr::V6(_)) => 6.0,
        Err(_) => 0.0,
    };
    Ok(Value::Number(family))
}

/// `net.isIPv4(input)`。
fn net_is_ipv4(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ip = net_is_ip(vm, args)?;
    Ok(Value::Boolean(matches!(ip, Value::Number(n) if n == 4.0)))
}

/// `net.isIPv6(input)`。
fn net_is_ipv6(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ip = net_is_ip(vm, args)?;
    Ok(Value::Boolean(matches!(ip, Value::Number(n) if n == 6.0)))
}

// --- BlockList ------------------------------------------------------------

/// `new net.BlockList()`：IP 黑名单实例（addAddress/addSubnet/addRange/check/rules）。
fn net_blocklist_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("net:blocklist".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in ["addAddress", "addSubnet", "addRange", "check"] {
        let fn_ref = vm.alloc_native_fn(&format!("net:blocklist.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    // rules 以快照数组属性呈现（每次变更后重写；原生 getter 不受支持，
    // 观察结果与 Go 的访问器一致）。
    refresh_blocklist_rules(vm, obj, &[]);
    Ok(Value::Object(obj))
}

/// 按 Go 语义重算并回写 `rules` 快照数组（Range → Subnet → Address 顺序）。
fn refresh_blocklist_rules(vm: &mut Vm, obj: ObjectRef, rules: &[BlockRule]) {
    let render = |rule: &BlockRule| match rule {
        BlockRule::Range(from, to) => {
            format!("Range: IPv4 {}-{}", u32_to_ipv4(*from), u32_to_ipv4(*to))
        }
        BlockRule::Subnet(net, prefix) => {
            format!("Subnet: IPv4 {}/{}", u32_to_ipv4(*net), prefix)
        }
        // 对齐 Go：地址条目恒标 IPv4（含原始串形态）。
        BlockRule::Address(addr) => format!("Address: IPv4 {addr}"),
    };
    // 对齐 Go rules getter：按 Range → Subnet → Address 分组输出（与插入序无关）。
    let mut vals: Vec<Value> = Vec::new();
    for rule in rules.iter().filter(|r| matches!(r, BlockRule::Range(..))) {
        vals.push(Value::Object(vm.alloc_string(render(rule))));
    }
    for rule in rules.iter().filter(|r| matches!(r, BlockRule::Subnet(..))) {
        vals.push(Value::Object(vm.alloc_string(render(rule))));
    }
    for rule in rules.iter().filter(|r| matches!(r, BlockRule::Address(_))) {
        vals.push(Value::Object(vm.alloc_string(render(rule))));
    }
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(Value::Object(obj), "rules", Value::Object(arr));
}

/// IPv4 u32 → 点分字符串（与 Go `IP.String()` 一致）。
fn u32_to_ipv4(v: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        v >> 24 & 0xFF,
        v >> 16 & 0xFF,
        v >> 8 & 0xFF,
        v & 0xFF
    )
}

/// IpAddr → IPv4 u32（大端权重，对齐 Go ipToUint32；非 IPv4 返回 None）。
fn ipv4_to_u32(ip: IpAddr) -> Option<u32> {
    match ip {
        IpAddr::V4(v4) => Some(u32::from_be_bytes(v4.octets())),
        IpAddr::V6(_) => None,
    }
}

/// `blocklist.addAddress(addr)`：存原始串（对齐 Go，不校验合法性）。
fn blocklist_add_address(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(obj) = receiver else {
        return Ok(receiver);
    };
    if let Some(addr) = args.first() {
        let text = vm.format_value(*addr);
        let snapshot = with_blocklist(obj.0, |rules| {
            rules.push(BlockRule::Address(text));
            rules.clone()
        });
        if let Some(rules) = snapshot {
            refresh_blocklist_rules(vm, obj, &rules);
        }
    }
    Ok(Value::Object(obj))
}

/// `blocklist.addSubnet(net, prefix)`：解析失败静默忽略（对齐 Go）。
fn blocklist_add_subnet(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(obj) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let net_text = vm.format_value(args[0]);
        let prefix = match args[1] {
            Value::Number(n) if (0.0..=32.0).contains(&n) => n as u32,
            _ => return Ok(Value::Object(obj)),
        };
        if let Some(base) = net_text.parse::<IpAddr>().ok().and_then(ipv4_to_u32) {
            let mask = subnet_mask(prefix);
            let snapshot = with_blocklist(obj.0, |rules| {
                rules.push(BlockRule::Subnet(base & mask, prefix));
                rules.clone()
            });
            if let Some(rules) = snapshot {
                refresh_blocklist_rules(vm, obj, &rules);
            }
        }
    }
    Ok(Value::Object(obj))
}

/// `blocklist.addRange(from, to)`：两端均可解析为 IPv4 时生效。
fn blocklist_add_range(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(obj) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let from = vm
            .format_value(args[0])
            .parse::<IpAddr>()
            .ok()
            .and_then(ipv4_to_u32);
        let to = vm
            .format_value(args[1])
            .parse::<IpAddr>()
            .ok()
            .and_then(ipv4_to_u32);
        if let (Some(from), Some(to)) = (from, to) {
            let snapshot = with_blocklist(obj.0, |rules| {
                rules.push(BlockRule::Range(from, to));
                rules.clone()
            });
            if let Some(rules) = snapshot {
                refresh_blocklist_rules(vm, obj, &rules);
            }
        }
    }
    Ok(Value::Object(obj))
}

/// `blocklist.check(ip)`：精确 → 子网 → 区间（仅 IPv4，对齐 Go）。
fn blocklist_check(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(obj) = receiver else {
        return Ok(receiver);
    };
    let Some(arg) = args.first() else {
        return Ok(Value::Boolean(false));
    };
    let Ok(ip) = vm.format_value(*arg).parse::<IpAddr>() else {
        return Ok(Value::Boolean(false));
    };
    let Some(target) = ipv4_to_u32(ip) else {
        return Ok(Value::Boolean(false));
    };
    // 对齐 Go：check 用规范化形式比对原始存储串。
    let canonical = ip.to_string();
    let hit = with_blocklist(obj.0, |rules| {
        rules.iter().any(|rule| match rule {
            BlockRule::Address(addr) => addr == &canonical,
            BlockRule::Subnet(net, prefix) => target & subnet_mask(*prefix) == *net,
            BlockRule::Range(from, to) => target >= *from && target <= *to,
        })
    })
    .unwrap_or(false);
    Ok(Value::Boolean(hit))
}

/// 前缀长度 → 掩码（prefix 0 → 0）。
fn subnet_mask(prefix: u32) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

// --- SocketAddress ----------------------------------------------------------

/// `new net.SocketAddress([options])`：地址描述对象（family 规范化为小写）。
fn net_socket_address_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let mut address = String::new();
    let mut port = 0.0f64;
    let mut family = "ipv4".to_owned();
    let mut flowlabel = 0.0f64;
    if let Some(opts) = args.first().copied().filter(|v| is_plain_object(vm, *v)) {
        if let Ok(v) = vm.get_property(opts, "address") {
            if !matches!(v, Value::Undefined) {
                address = vm.format_value(v);
            }
        }
        if let Ok(Value::Number(n)) = vm.get_property(opts, "port") {
            port = n;
        }
        if let Ok(v) = vm.get_property(opts, "family") {
            if !matches!(v, Value::Undefined) {
                family = vm.format_value(v).to_lowercase();
            }
        }
        if let Ok(Value::Number(n)) = vm.get_property(opts, "flowlabel") {
            flowlabel = n;
        }
    }
    let s_alloc0 = vm.alloc_string(address);
    let _ = vm.set_property(Value::Object(obj), "address", Value::Object(s_alloc0));
    let _ = vm.set_property(Value::Object(obj), "port", Value::Number(port));
    let s_alloc0 = vm.alloc_string(family);
    let _ = vm.set_property(Value::Object(obj), "family", Value::Object(s_alloc0));
    let _ = vm.set_property(Value::Object(obj), "flowlabel", Value::Number(flowlabel));
    Ok(Value::Object(obj))
}

// --- 实例方法处理器 ---------------------------------------------------------

/// `on(event, listener)` / `addListener`。
fn net_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_listeners(r.0, |ls| {
            ls.entry(event)
                .or_default()
                .push(NetListener { cb, once: false });
        });
    }
    Ok(receiver)
}

/// `once(event, listener)`。
fn net_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_listeners(r.0, |ls| {
            ls.entry(event)
                .or_default()
                .push(NetListener { cb, once: true });
        });
    }
    Ok(receiver)
}

/// `emit(event, ...args)`（用户侧触发，语义与泵内派发一致）。
fn net_emit_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(event) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Boolean(false));
    };
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
    emit_net_event(vm, receiver, &event, &emit_args)?;
    Ok(Value::Boolean(true))
}

/// `off(event, listener)` / `removeListener`（按句柄移除首个匹配）。
fn net_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_listeners(r.0, |ls| {
            if let Some(list) = ls.get_mut(&event) {
                if let Some(pos) = list.iter().position(|l| is_same_value(l.cb, cb)) {
                    list.remove(pos);
                }
            }
        });
    }
    Ok(receiver)
}

/// `listenerCount(event)`。
fn net_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Number(0.0));
    };
    let Some(event) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Number(0.0));
    };
    let count = with_listeners(r.0, |ls| ls.get(&event).map(|l| l.len()).unwrap_or(0)).unwrap_or(0);
    Ok(Value::Number(count as f64))
}

/// 监听器移除用的值同一性（对象比句柄；与 events 模块语义一致）。
fn is_same_value(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

/// `socket.write(data[, encoding][, callback])`：同步写出，回调异步触发。
fn net_socket_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Boolean(false));
    };
    let Some(data) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let write_cb = args
        .iter()
        .skip(1)
        .rev()
        .find(|a| is_function(vm, **a))
        .copied();
    let bytes = extract_bytes(vm, data).unwrap_or_else(|| vm.format_value(data).into_bytes());
    with_net(|n| {
        let Some((_, state)) = n.sockets.iter_mut().find(|(id, _)| *id == r.0) else {
            return Ok(Value::Boolean(false));
        };
        if state.closed {
            return Ok(Value::Boolean(false));
        }
        let Some(stream) = state.stream.as_mut() else {
            return Ok(Value::Boolean(false));
        };
        match stream.write_all(&bytes) {
            Ok(()) => {
                if let Some(cb) = write_cb {
                    n.pending.push_back(NetAction::Call {
                        cb,
                        this: receiver,
                        args: Vec::new(),
                    });
                }
                Ok(Value::Boolean(true))
            }
            // 对齐 Go：写出失败以异常上抛。
            Err(e) => Err(VmError::Thrown(Value::Object(vm_alloc_error(
                vm,
                &e.to_string(),
            )))),
        }
    })
}

/// 在堆上分配 Error 实例（消息载体）。
fn vm_alloc_error(vm: &mut Vm, message: &str) -> ObjectRef {
    vm.alloc_error_instance(message)
}

/// `socket.end([data])`：可选写出后关闭（'end'/'close' 异步成对派发）。
fn net_socket_end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let Some(data) = args.first().copied() {
        if !matches!(data, Value::Undefined | Value::Null) {
            let bytes =
                extract_bytes(vm, data).unwrap_or_else(|| vm.format_value(data).into_bytes());
            with_net(|n| {
                if let Some((_, state)) = n.sockets.iter_mut().find(|(id, _)| *id == r.0) {
                    if !state.closed {
                        if let Some(stream) = state.stream.as_mut() {
                            let _ = stream.write_all(&bytes);
                        }
                    }
                }
            });
        }
    }
    close_socket_lifecycle(r.0);
    Ok(receiver)
}

/// `socket.destroy()`：立即关闭并派发生命周期事件。
fn net_socket_destroy(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        close_socket_lifecycle(r.0);
    }
    Ok(receiver)
}

/// 关闭 socket 并入队 `'end'` + `'close'`（幂等；实例随即从轮询表移除，
/// 派发所需数据由动作自带，对齐 Go 读循环收尾的 PostTask 派发）。
fn close_socket_lifecycle(id: u32) {
    with_net(|n| {
        let Some(pos) = n.sockets.iter().position(|(sid, _)| *sid == id) else {
            return;
        };
        let (_, mut state) = n.sockets.remove(pos);
        if state.closed {
            return;
        }
        state.closed = true;
        state.stream = None;
        let obj = state.obj;
        // 实例即将移除：监听器先行快照进动作（对齐 Go PostTask 携带闭包）。
        let end_listeners = drain_listeners(&mut state.listeners, "end");
        let close_listeners = drain_listeners(&mut state.listeners, "close");
        n.pending.push_back(NetAction::EmitWith {
            target: obj,
            event: "end".to_owned(),
            args: Vec::new(),
            listeners: end_listeners,
        });
        n.pending.push_back(NetAction::EmitWith {
            target: obj,
            event: "close".to_owned(),
            args: Vec::new(),
            listeners: close_listeners,
        });
    });
}

/// 取走指定事件的监听器快照（once 语义在快照中已含，直接全部触发）。
fn drain_listeners(listeners: &mut HashMap<String, Vec<NetListener>>, event: &str) -> Vec<Value> {
    listeners
        .remove(event)
        .map(|list| list.into_iter().map(|l| l.cb).collect())
        .unwrap_or_default()
}

/// `socket.address()`：本地地址对象（未连接时空对象，对齐 Go）。
fn net_socket_address(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let local = with_net(|n| {
        n.sockets
            .iter()
            .find(|(id, _)| *id == r.0)
            .and_then(|(_, s)| s.stream.as_ref())
            .and_then(|s| s.local_addr().ok())
    });
    let obj = vm.alloc_ordinary();
    if let Some(addr) = local {
        let s_alloc0 = vm.alloc_string(addr.ip().to_string());
        let _ = vm.set_property(Value::Object(obj), "address", Value::Object(s_alloc0));
        let _ = vm.set_property(
            Value::Object(obj),
            "port",
            Value::Number(addr.port() as f64),
        );
    }
    Ok(Value::Object(obj))
}

/// `socket.pipe(dest)`：把后续 `'data'` 事件转发到 `dest.write(chunk)`。
fn net_socket_pipe(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let Some(dest) = args.first().copied() {
        if matches!(dest, Value::Object(_)) {
            with_net(|n| {
                if let Some((_, state)) = n.sockets.iter_mut().find(|(id, _)| *id == r.0) {
                    state.pipe_dest = Some(dest);
                }
            });
        }
    }
    Ok(receiver)
}

/// setEncoding / setNoDelay / setTimeout / setKeepAlive / ref / unref /
/// pause / resume：兼容性 no-op，返回自身。
fn net_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

// --- Server 方法 -----------------------------------------------------------

/// `server.listen(port[, host][, callback])`：同步绑定，事件异步派发。
fn net_server_listen(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let port = match args.first() {
        Some(Value::Number(n)) => *n as u16,
        _ => 0,
    };
    let mut host = String::new();
    let mut callback: Option<Value> = None;
    if let Some(second) = args.get(1).copied() {
        if is_function(vm, second) {
            callback = Some(second);
        } else {
            host = vm.format_value(second);
            if let Some(third) = args.get(2).copied() {
                if is_function(vm, third) {
                    callback = Some(third);
                }
            }
        }
    }
    let bind_host = if host.is_empty() {
        "0.0.0.0".to_owned()
    } else {
        host
    };
    match TcpListener::bind((bind_host.as_str(), port)) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            let bound = listener.local_addr().ok();
            with_net(|n| {
                if let Some((_, state)) = n.servers.iter_mut().find(|(id, _)| *id == r.0) {
                    state.listener = Some(listener);
                    state.bound_addr = bound;
                }
            });
            let _ = vm.set_property(Value::Object(r), "listening", Value::Boolean(true));
            if let Some(cb) = callback {
                with_net(|n| {
                    n.pending.push_back(NetAction::Call {
                        cb,
                        this: Value::Object(r),
                        args: Vec::new(),
                    });
                });
            }
            with_net(|n| {
                n.pending.push_back(NetAction::Emit {
                    target: Value::Object(r),
                    event: "listening".to_owned(),
                    args: Vec::new(),
                });
            });
            vm.activate_event_source("net", net_pump);
        }
        Err(e) => {
            // 对齐 Go：监听失败异步派发 'error'（OS 错误文案）。
            let err_val = Value::Object(vm.alloc_string(e.to_string()));
            with_net(|n| {
                n.pending.push_back(NetAction::Emit {
                    target: Value::Object(r),
                    event: "error".to_owned(),
                    args: vec![err_val],
                });
            });
            vm.activate_event_source("net", net_pump);
        }
    }
    Ok(Value::Object(r))
}

/// `server.close([callback])`：停止 accept，close 事件先于回调异步派发。
fn net_server_close(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let callback = args.first().copied().filter(|v| is_function(vm, *v));
    let close_action = with_net(|n| {
        let (_, state) = n.servers.iter_mut().find(|(id, _)| *id == r.0)?;
        if state.closed {
            return None;
        }
        state.closed = true;
        state.listener = None; // drop 关闭监听器，accept 轮询随之终结；
        // 保留条目（bound_addr 供 close 后 address() 使用）
        // close 监听器先行快照进动作（派发时事件语义不受条目保留影响）。
        let listeners: Vec<Value> = state
            .listeners
            .remove("close")
            .map(|list| list.into_iter().map(|l| l.cb).collect())
            .unwrap_or_default();
        let server_obj = state.obj;
        let mut actions = vec![NetAction::EmitWith {
            target: server_obj,
            event: "close".to_owned(),
            args: Vec::new(),
            listeners,
        }];
        if let Some(cb) = callback {
            actions.push(NetAction::Call {
                cb,
                this: Value::Undefined,
                args: Vec::new(),
            });
        }
        Some(NetAction::Composite(actions))
    });
    if let Some(action) = close_action {
        with_net(|n| n.pending.push_back(action));
        vm.activate_event_source("net", net_pump);
    } else if let Some(cb) = callback {
        // 对齐 Go：未监听时回调同步执行、不派发 close 事件。
        vm.invoke_callable(cb, Value::Undefined, &[])?;
    }
    Ok(Value::Object(r))
}

/// `server.address()`：`{address, port, family}`（未监听时 null，对齐 Go）。
fn net_server_address(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let bound = with_net(|n| {
        n.servers
            .iter()
            .find(|(id, _)| *id == r.0)
            .and_then(|(_, s)| s.bound_addr)
    });
    let Some(addr) = bound else {
        return Ok(Value::Null);
    };
    let obj = vm.alloc_ordinary();
    let s_alloc0 = vm.alloc_string(addr.ip().to_string());
    let _ = vm.set_property(Value::Object(obj), "address", Value::Object(s_alloc0));
    let _ = vm.set_property(
        Value::Object(obj),
        "port",
        Value::Number(addr.port() as f64),
    );
    let s_alloc0 = vm.alloc_string("IPv4".to_owned());
    let _ = vm.set_property(Value::Object(obj), "family", Value::Object(s_alloc0));
    Ok(Value::Object(obj))
}

/// `server.getConnections(cb)`：当前连接数（简化恒 0，对齐 Go）。
fn net_server_get_connections(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Some(cb) = args.first().copied() {
        if is_function(vm, cb) {
            vm.invoke_callable(cb, Value::Undefined, &[Value::Null, Value::Number(0.0)])?;
        }
    }
    Ok(receiver)
}

// --- 事件源泵 ---------------------------------------------------------------

/// net 事件源泵：完成挂起连接 → accept 轮询 → 统一派发 → 读轮询 → 派发。
/// accept 轮询先于派发，使「连接回调 → 对端 'connection' → 连接回调里
/// 入队的写回调」保持 Go 任务队列的 FIFO 观测顺序；全部实体终结且队列
/// 排空时自动注销事件源。
fn net_pump(vm: &mut Vm) -> Result<bool, VmError> {
    let idle = with_net(|n| {
        n.pending.is_empty()
            && n.sockets.is_empty()
            && n.servers
                .iter()
                .all(|(_, s)| s.closed || s.listener.is_none())
    });
    if idle {
        vm.deactivate_event_source("net");
        return Ok(false);
    }
    let mut progressed = false;

    // 1. 完成挂起的客户端连接（回环地址毫秒级，同步拨号可接受）。
    let connects: Vec<(u32, String, u16)> = with_net(|n| {
        n.sockets
            .iter()
            .filter(|(_, s)| !s.closed)
            .filter_map(|(id, s)| s.connecting.as_ref().map(|(h, p)| (*id, h.clone(), *p)))
            .collect()
    });
    for (id, host, port) in connects {
        match TcpStream::connect((host.as_str(), port)) {
            Ok(stream) => {
                let _ = stream.set_nonblocking(true);
                let (obj_val, connect_seq) = with_net(|n| {
                    let Some((_, state)) = n.sockets.iter_mut().find(|(sid, _)| *sid == id) else {
                        return (Value::Undefined, None);
                    };
                    state.stream = Some(stream);
                    state.connecting = None;
                    let obj = state.obj;
                    let mut actions = Vec::new();
                    // 对齐 Go：连接监听器先于 'connect' 事件派发。
                    if let Some(cb) = state.connect_listener.take() {
                        actions.push(NetAction::Call {
                            cb,
                            this: obj,
                            args: vec![obj],
                        });
                    }
                    actions.push(NetAction::Emit {
                        target: obj,
                        event: "connect".to_owned(),
                        args: Vec::new(),
                    });
                    (obj, Some(NetAction::Composite(actions)))
                });
                if let (Value::Object(r), Some(cloned)) = (
                    obj_val,
                    with_net(|n| {
                        n.sockets
                            .iter()
                            .find(|(sid, _)| *sid == id)
                            .and_then(|(_, s)| s.stream.as_ref())
                            .and_then(|s| s.try_clone().ok())
                    }),
                ) {
                    set_addr_props(vm, r, &cloned);
                }
                if let Some(seq) = connect_seq {
                    with_net(|n| n.pending.push_back(seq));
                }
                progressed = true;
            }
            Err(e) => {
                // 拨号失败：异步派发 'error'（OS 文案）后 socket 终结（无 end/close）。
                let err_val = Value::Object(vm.alloc_string(e.to_string()));
                let emit = with_net(|n| {
                    let pos = n.sockets.iter().position(|(sid, _)| *sid == id)?;
                    let (_, mut state) = n.sockets.remove(pos);
                    let listeners = drain_listeners(&mut state.listeners, "error");
                    Some(NetAction::EmitWith {
                        target: state.obj,
                        event: "error".to_owned(),
                        args: vec![err_val],
                        listeners,
                    })
                });
                if let Some(action) = emit {
                    with_net(|n| n.pending.push_back(action));
                }
                progressed = true;
            }
        }
    }

    // 2. accept 轮询先于派发：连接序列与 'connection' 序列按 Go 任务队列
    //    顺序（先连接回调、后对端 connection）一次性入队。
    // accept 轮询：新连接 → socket 实例 → 'connection' 事件 + 连接监听器。
    let server_ids: Vec<u32> = with_net(|n| n.servers.iter().map(|(id, _)| *id).collect());
    for sid in server_ids {
        loop {
            let accepted = with_net(|n| {
                let (_, state) = n.servers.iter_mut().find(|(id, _)| *id == sid)?;
                let listener = state.listener.as_mut()?;
                match listener.accept() {
                    Ok((stream, _peer)) => Some((stream, state.conn_listener, state.obj)),
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => None,
                    Err(_) => None, // 监听器故障：停止 accept（对齐 Go break）
                }
            });
            let Some((stream, conn_listener, server_obj)) = accepted else {
                break;
            };
            let _ = stream.set_nonblocking(true);
            let sock_obj = create_socket_instance(vm);
            set_addr_props(vm, sock_obj, &stream);
            with_net(|n| {
                n.sockets.push((
                    sock_obj.0,
                    SocketState {
                        obj: Value::Object(sock_obj),
                        stream: Some(stream),
                        connecting: None,
                        closed: false,
                        connect_listener: None,
                        listeners: HashMap::new(),
                        pipe_dest: None,
                    },
                ));
                let mut actions = vec![NetAction::Emit {
                    target: server_obj,
                    event: "connection".to_owned(),
                    args: vec![Value::Object(sock_obj)],
                }];
                if let Some(cb) = conn_listener {
                    actions.push(NetAction::Call {
                        cb,
                        this: Value::Undefined,
                        args: vec![Value::Object(sock_obj)],
                    });
                }
                n.pending.push_back(NetAction::Composite(actions));
            });
            progressed = true;
        }
    }

    // 3. 统一派发（connect 序列 → connection 序列 → 其间入队的回调），
    //    连接处理器得以注册 data 监听。
    progressed |= dispatch_pending(vm)?;

    // 4. 读轮询：数据 → 'data'（Buffer）；EOF / 错误 → end/close 生命周期。
    let socket_ids: Vec<u32> = with_net(|n| n.sockets.iter().map(|(id, _)| *id).collect());
    for sock_id in socket_ids {
        let read_result = with_net(|n| {
            let (_, state) = n.sockets.iter_mut().find(|(id, _)| *id == sock_id)?;
            let stream = state.stream.as_mut()?;
            let mut buf = vec![0u8; READ_CHUNK];
            match stream.read(&mut buf) {
                Ok(0) => Some(None),
                Ok(n) => Some(Some(buf[..n].to_vec())),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => None,
                Err(_) => Some(None),
            }
        });
        let Some(chunk) = read_result else {
            continue;
        };
        progressed = true;
        match chunk {
            Some(bytes) => {
                let buf_obj = create_buffer_instance(vm, bytes);
                let obj_val = with_net(|n| {
                    n.sockets
                        .iter()
                        .find(|(id, _)| *id == sock_id)
                        .map(|(_, s)| s.obj)
                });
                if let Some(obj) = obj_val {
                    with_net(|n| {
                        n.pending.push_back(NetAction::Emit {
                            target: obj,
                            event: "data".to_owned(),
                            args: vec![Value::Object(buf_obj)],
                        });
                    });
                }
            }
            None => close_socket_lifecycle(sock_id),
        }
    }

    // 5. 派发 data / 生命周期。
    progressed |= dispatch_pending(vm)?;
    Ok(progressed)
}

/// 排空待派发队列；回调可继续入队（写回调、close 序列等），循环至队列清空。
fn dispatch_pending(vm: &mut Vm) -> Result<bool, VmError> {
    let mut any = false;
    loop {
        let action = with_net(|n| n.pending.pop_front());
        let Some(action) = action else {
            break;
        };
        any = true;
        exec_action(vm, action)?;
    }
    Ok(any)
}

/// 解释执行单个待派发动作。
fn exec_action(vm: &mut Vm, action: NetAction) -> Result<(), VmError> {
    match action {
        NetAction::Composite(items) => {
            for item in items {
                exec_action(vm, item)?;
            }
            Ok(())
        }
        NetAction::Emit {
            target,
            event,
            args,
        } => emit_net_event(vm, target, &event, &args),
        NetAction::Call { cb, this, args } => {
            vm.invoke_callable(cb, this, &args)?;
            Ok(())
        }
        NetAction::EmitWith {
            target,
            event,
            args,
            listeners,
        } => {
            if listeners.is_empty() && event == "error" {
                let err_val = args.first().copied().unwrap_or(Value::Undefined);
                return Err(VmError::Thrown(err_val));
            }
            for cb in listeners {
                vm.invoke_callable(cb, target, &args)?;
            }
            Ok(())
        }
    }
}

/// 触发 net 实例事件：快照监听器（once 移除）后逐个调用；
/// 无监听器的 'error' 按未捕获异常上抛（对齐 EventEmitter 语义）；
/// 'data' 事件在设置 pipe 目标时转发到 `dest.write(chunk)`。
fn emit_net_event(vm: &mut Vm, target: Value, event: &str, args: &[Value]) -> Result<(), VmError> {
    let Value::Object(r) = target else {
        return Ok(());
    };
    let listeners = with_listeners(r.0, |ls| {
        let Some(list) = ls.get_mut(event) else {
            return Vec::new();
        };
        let mut to_call = Vec::new();
        let mut keep = Vec::with_capacity(list.len());
        for item in list.drain(..) {
            to_call.push(item.cb);
            if !item.once {
                keep.push(item);
            }
        }
        *list = keep;
        to_call
    })
    .unwrap_or_default();

    if listeners.is_empty() && event == "error" {
        let err_val = args.first().copied().unwrap_or(Value::Undefined);
        return Err(VmError::Thrown(err_val));
    }
    for cb in listeners {
        vm.invoke_callable(cb, target, args)?;
    }
    if event == "data" {
        let dest = with_net(|n| {
            n.sockets
                .iter()
                .find(|(id, _)| *id == r.0)
                .and_then(|(_, s)| s.pipe_dest)
        });
        if let Some(dest) = dest {
            if let Ok(write_fn) = vm.get_property(dest, "write") {
                vm.invoke_callable(write_fn, dest, args)?;
            }
        }
    }
    Ok(())
}
