//! `node:dgram` 内置模块（Phase 5）：UDP 数据报。
//!
//! 与 Node.js 22 LTS 标准（`nodenet/dgram.go`）对齐：
//! - `dgram.createSocket(type[, callback])`（'udp4'/'udp6'，callback 注册为
//!   'message' 监听器）；`dgram.Socket` 构造器；
//! - Socket 方法：`bind` / `send` / `close` / `address` / `connect` /
//!   `disconnect` 与兼容 no-op（setBroadcast/addMembership/dropMembership/
//!   setTTL/setMulticastLoopback/setMulticastTTL/ref/unref）+ 事件族；
//! - UDP 真实回环收发：`std::net::UdpSocket`（非阻塞）；
//! - `send` 支持隐式绑定（未 bind 直接发送，自动绑定临时端口）；
//! - `bind` 成功后异步派发 `'listening'` 再调用回调（对齐 Go PostTask：
//!   listening 事件监听器先执行、bind 回调后执行）；
//! - `'message'` 事件携带 `(Buffer, rinfo{address, port, family, size})`。
//!
//! 异步时序：接收经 `activate_event_source("dgram", dgram_pump)` 泵轮询；
//! 全部 socket 关闭且队列排空后自动注销事件源，进程正常退出。

use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};

/// UDP 单次接收缓冲上限（对齐 Go 65536 字节缓冲）。
const RECV_CHUNK: usize = 65536;

/// 待派发动作（dgram 仅需事件与回调两类）。
enum DgramAction {
    /// 触发 socket 实例事件（无监听器的 'error' 上抛未捕获异常）。
    Emit {
        /// 目标 socket 对象值。
        target: Value,
        /// 事件名。
        event: String,
        /// 事件实参。
        args: Vec<Value>,
    },
    /// 调用回调。
    Call {
        /// 回调值。
        cb: Value,
        /// 实参。
        args: Vec<Value>,
    },
}

/// UDP Socket 实例共享状态。
struct DgramSocketState {
    /// 实例对象值（派发事件时的 this）。
    obj: Value,
    /// 地址族（'udp4' / 'udp6'）。
    net_type: String,
    /// 已绑定的 UDP 套接字（非阻塞）。
    socket: Option<UdpSocket>,
    /// 绑定的本地地址。
    bound_addr: Option<SocketAddr>,
    /// `connect()` 设置的默认发送目标。
    connected_dest: Option<SocketAddr>,
    /// 事件监听器表。
    listeners: HashMap<String, Vec<DgramListener>>,
}

/// 事件监听器条目。
struct DgramListener {
    /// 监听器回调。
    cb: Value,
    /// 是否单次触发。
    once: bool,
}

/// dgram 模块共享状态。
#[derive(Default)]
struct DgramShared {
    /// 存活 socket（按创建序）。
    sockets: Vec<(u32, DgramSocketState)>,
    /// 待派发队列。
    pending: VecDeque<DgramAction>,
}

// dgram 模块共享状态（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static DGRAM_SHARED: RefCell<Option<DgramShared>> = const { RefCell::new(None) };
}

/// 在线程局部借用内访问 dgram 共享状态（闭包内禁止触碰 `Vm`）。
fn with_dgram<R>(f: impl FnOnce(&mut DgramShared) -> R) -> R {
    DGRAM_SHARED.with(|g| f(g.borrow_mut().get_or_insert_with(DgramShared::default)))
}

/// `require("dgram")` / `require("node:dgram")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "dgram",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    let create_fn = vm.alloc_native_fn("dgram.createSocket");
    set_module_prop(vm, obj, "createSocket", Value::Object(create_fn))?;
    let socket_ctor = vm.alloc_native_fn("dgram.Socket");
    set_module_prop(vm, obj, "Socket", Value::Object(socket_ctor))?;

    register_handler(registry, "dgram", "createSocket", dgram_create_socket);
    register_handler(registry, "dgram", "Socket", dgram_socket_ctor);

    let table: &[(&str, BuiltinHandler)] = &[
        ("on", dgram_on),
        ("addListener", dgram_on),
        ("once", dgram_once),
        ("emit", dgram_emit_handler),
        ("off", dgram_off),
        ("removeListener", dgram_off),
        ("listenerCount", dgram_listener_count),
        ("bind", dgram_bind),
        ("send", dgram_send),
        ("close", dgram_close),
        ("address", dgram_address),
        ("connect", dgram_connect),
        ("disconnect", dgram_disconnect),
        ("setBroadcast", dgram_noop_self),
        ("addMembership", dgram_noop_self),
        ("dropMembership", dgram_noop_self),
        ("setTTL", dgram_noop_self),
        ("setMulticastLoopback", dgram_noop_self),
        ("setMulticastTTL", dgram_noop_self),
        ("ref", dgram_noop_self),
        ("unref", dgram_noop_self),
    ];
    for (method, handler) in table {
        register_handler(registry, "dgram:socket", method, *handler);
    }

    Ok(obj)
}

/// 判断值是否为可调用对象。
fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
    ))
}

/// 创建 UDP socket 实例（net_type 默认 udp4；message_cb 注册为 'message' 监听器）。
fn create_dgram_socket(vm: &mut Vm, net_type: &str, message_cb: Option<Value>) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("dgram:socket".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in [
        "bind",
        "send",
        "close",
        "address",
        "connect",
        "disconnect",
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "listenerCount",
        "setBroadcast",
        "addMembership",
        "dropMembership",
        "setTTL",
        "setMulticastLoopback",
        "setMulticastTTL",
        "ref",
        "unref",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("dgram:socket.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    let mut listeners: HashMap<String, Vec<DgramListener>> = HashMap::new();
    if let Some(cb) = message_cb {
        listeners
            .entry("message".to_owned())
            .or_default()
            .push(DgramListener { cb, once: false });
    }
    with_dgram(|s| {
        s.sockets.push((
            obj.0,
            DgramSocketState {
                obj: Value::Object(obj),
                net_type: net_type.to_owned(),
                socket: None,
                bound_addr: None,
                connected_dest: None,
                listeners,
            },
        ));
    });
    obj
}

/// `dgram.createSocket(type[, callback])`。
fn dgram_create_socket(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut net_type = "udp4".to_owned();
    if let Some(first) = args.first() {
        if matches!(first, Value::Object(_)) {
            let text = vm.format_value(*first);
            if !text.is_empty() {
                net_type = text;
            }
        }
    }
    let message_cb = args.get(1).copied().filter(|v| is_function(vm, *v));
    let obj = create_dgram_socket(vm, &net_type, message_cb);
    Ok(Value::Object(obj))
}

/// `new dgram.Socket()`：默认 udp4 未绑定实例（对齐 Go 构造器）。
fn dgram_socket_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let obj = create_dgram_socket(vm, "udp4", None);
    Ok(Value::Object(obj))
}

// --- 事件族 ---------------------------------------------------------------

/// `on(event, listener)` / `addListener`。
fn dgram_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_socket_listeners(r.0, |ls| {
            ls.entry(event)
                .or_default()
                .push(DgramListener { cb, once: false });
        });
    }
    Ok(receiver)
}

/// `once(event, listener)`。
fn dgram_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_socket_listeners(r.0, |ls| {
            ls.entry(event)
                .or_default()
                .push(DgramListener { cb, once: true });
        });
    }
    Ok(receiver)
}

/// `emit(event, ...args)`（用户侧触发）。
fn dgram_emit_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(event) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Boolean(false));
    };
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
    emit_dgram_event(vm, receiver, &event, &emit_args)?;
    Ok(Value::Boolean(true))
}

/// `off(event, listener)` / `removeListener`。
fn dgram_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if args.len() >= 2 {
        let event = vm.format_value(args[0]);
        let cb = args[1];
        with_socket_listeners(r.0, |ls| {
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
fn dgram_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Number(0.0));
    };
    let Some(event) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Number(0.0));
    };
    let count =
        with_socket_listeners(r.0, |ls| ls.get(&event).map(|l| l.len()).unwrap_or(0)).unwrap_or(0);
    Ok(Value::Number(count as f64))
}

/// 监听器移除用的值同一性（对象比句柄）。
fn is_same_value(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

/// 在线程局部借用内访问指定 socket 的监听器表。
fn with_socket_listeners<R>(
    id: u32,
    f: impl FnOnce(&mut HashMap<String, Vec<DgramListener>>) -> R,
) -> Option<R> {
    with_dgram(|s| {
        s.sockets
            .iter_mut()
            .find(|(sid, _)| *sid == id)
            .map(|(_, state)| f(&mut state.listeners))
    })
}

/// 触发 dgram socket 事件：快照监听器（once 移除）后逐个调用；
/// 无监听器的 'error' 按未捕获异常上抛。
fn emit_dgram_event(
    vm: &mut Vm,
    target: Value,
    event: &str,
    args: &[Value],
) -> Result<(), VmError> {
    let Value::Object(r) = target else {
        return Ok(());
    };
    let listeners = with_socket_listeners(r.0, |ls| {
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
    Ok(())
}

// --- Socket 方法 -----------------------------------------------------------

/// `socket.bind([port][, address][, callback])`：任意顺序解析
/// （数字 → port，字符串 → address，函数 → callback，对齐 Go）。
fn dgram_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let mut port = 0u16;
    let mut address = String::new();
    let mut cb: Option<Value> = None;
    for a in args {
        if is_function(vm, *a) {
            cb = Some(*a);
        } else if let Some(n) = a.as_number {
            port = *n as u16;
        } else if matches!(a, Value::Object(_)) {
            address = vm.format_value(*a);
        }
    }
    let net_type = with_dgram(|s| {
        s.sockets
            .iter()
            .find(|(sid, _)| *sid == r.0)
            .map(|(_, st)| st.net_type.clone())
            .unwrap_or_else(|| "udp4".to_owned())
    });
    if address.is_empty() {
        address = if net_type == "udp6" {
            "::".to_owned()
        } else {
            "0.0.0.0".to_owned()
        };
    }
    match UdpSocket::bind((address.as_str(), port)) {
        Ok(socket) => {
            let _ = socket.set_nonblocking(true);
            let local = socket.local_addr().ok();
            with_dgram(|s| {
                if let Some((_, state)) = s.sockets.iter_mut().find(|(sid, _)| *sid == r.0) {
                    state.socket = Some(socket);
                    state.bound_addr = local;
                }
                // 对齐 Go：先派发 'listening' 事件（监听器内可完成收发全流程），
                // 之后才执行 bind 回调。
                s.pending.push_back(DgramAction::Emit {
                    target: Value::Object(r),
                    event: "listening".to_owned(),
                    args: Vec::new(),
                });
                if let Some(cb) = cb {
                    s.pending.push_back(DgramAction::Call {
                        cb,
                        args: Vec::new(),
                    });
                }
            });
            vm.activate_event_source("dgram", dgram_pump);
        }
        Err(e) => {
            // 对齐 Go：bind 失败同步派发 'error'（OS 文案）。
            let err_val = Value::Object(vm.alloc_string(e.to_string()));
            emit_dgram_event(vm, Value::Object(r), "error", &[err_val])?;
        }
    }
    Ok(Value::Object(r))
}

/// `socket.send(msg[, offset, length][, port][, address][, callback])`。
/// 支持未绑定 socket 的隐式绑定（自动绑定临时端口，对齐 Node/Go）。
fn dgram_send(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let Some(data) = args.first().copied() else {
        let err = vm.alloc_error_instance("send: message required");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let bytes = extract_bytes(vm, data).unwrap_or_else(|| vm.format_value(data).into_bytes());
    let mut port = 0u16;
    let mut address = String::new();
    let mut cb: Option<Value> = None;
    for a in args.iter().skip(1) {
        if is_function(vm, *a) {
            cb = Some(*a);
        } else if let Some(n) = a.as_number {
            port = *n as u16;
        } else if matches!(a, Value::Object(_)) {
            address = vm.format_value(*a);
        }
    }

    let (net_type, has_socket, bound_ip, connected) = with_dgram(|s| {
        let Some((_, state)) = s.sockets.iter_mut().find(|(sid, _)| *sid == r.0) else {
            return ("udp4".to_owned(), false, None::<IpAddr>, None);
        };
        (
            state.net_type.clone(),
            state.socket.is_some(),
            state.bound_addr.map(|a| a.ip()),
            state.connected_dest,
        )
    });

    // 隐式绑定：未 bind 的 socket 直接发送时绑定临时端口。
    if !has_socket {
        let bind_ip = if net_type == "udp6" { "::" } else { "0.0.0.0" };
        match UdpSocket::bind((bind_ip, 0)) {
            Ok(socket) => {
                let _ = socket.set_nonblocking(true);
                let local = socket.local_addr().ok();
                with_dgram(|s| {
                    if let Some((_, state)) = s.sockets.iter_mut().find(|(sid, _)| *sid == r.0) {
                        state.socket = Some(socket);
                        state.bound_addr = local;
                    }
                });
                vm.activate_event_source("dgram", dgram_pump);
            }
            Err(e) => {
                // 隐式绑定失败：同步以错误串回调（对齐 Go lerr 上抛路径的观测形态）。
                if let Some(cb) = cb {
                    let err_text = Value::Object(vm.alloc_string(e.to_string()));
                    vm.invoke_callable(cb, Value::Undefined, &[err_text])?;
                }
                return Ok(Value::Object(r));
            }
        }
    }

    // 目标解析：显式地址+端口 → 仅端口（用绑定 IP）→ connected 目标。
    let dest: Option<SocketAddr> = if !address.is_empty() && port > 0 {
        (address.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut i| i.next())
    } else if port > 0 {
        let ip = bound_ip.unwrap_or_else(|| "127.0.0.1".parse().expect("恒成立"));
        SocketAddr::new(ip, port).into()
    } else {
        connected
    };

    let Some(dest) = dest else {
        // 对齐 Go：无目标时同步以错误串回调。
        if let Some(cb) = cb {
            let not_bound = Value::Object(vm.alloc_string("socket not bound".to_owned()));
            vm.invoke_callable(cb, Value::Undefined, &[not_bound])?;
        }
        return Ok(Value::Object(r));
    };

    let send_err = with_dgram(|s| {
        s.sockets
            .iter_mut()
            .find(|(sid, _)| *sid == r.0)
            .and_then(|(_, state)| state.socket.as_mut())
            .map(|sock| sock.send_to(&bytes, dest).err())
            .unwrap_or(Some(std::io::Error::new(
                ErrorKind::NotConnected,
                "socket closed",
            )))
    });
    // 对齐 Go：回调同步执行；成功 (null)，失败 (错误字符串)。
    if let Some(cb) = cb {
        match send_err {
            Some(e) => {
                let err_text = Value::Object(vm.alloc_string(e.to_string()));
                vm.invoke_callable(cb, Value::Undefined, &[err_text])?;
            }
            None => {
                vm.invoke_callable(cb, Value::Undefined, &[Value::Null])?;
            }
        }
    }
    Ok(Value::Object(r))
}

/// `socket.close([callback])`：同步关闭并派发 'close'（对齐 Go）。
fn dgram_close(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let cb = args.first().copied().filter(|v| is_function(vm, *v));
    // 首次 close：移出轮询表（关闭套接字）、同步回调并派发 'close'；
    // 重复 close 为 no-op（对齐 Go closed 标记幂等）。
    let already_closed = with_dgram(|s| {
        let Some(pos) = s.sockets.iter().position(|(sid, _)| *sid == r.0) else {
            return true;
        };
        s.sockets.remove(pos);
        false
    });
    if already_closed {
        return Ok(Value::Object(r));
    }
    if let Some(cb) = cb {
        vm.invoke_callable(cb, Value::Undefined, &[])?;
    }
    emit_dgram_event(vm, Value::Object(r), "close", &[])?;
    Ok(Value::Object(r))
}

/// `socket.address()`：`{address, port, family}`；未绑定时抛错（对齐 Go）。
fn dgram_address(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let local = with_dgram(|s| {
        s.sockets
            .iter()
            .find(|(sid, _)| *sid == r.0)
            .and_then(|(_, state)| state.socket.as_ref())
            .and_then(|sock| sock.local_addr().ok())
    });
    let Some(addr) = local else {
        let err = vm.alloc_error_instance("not bound");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let obj = vm.alloc_ordinary();
    let s_alloc0 = vm.alloc_string(addr.ip().to_string());
    let _ = vm.set_property(Value::Object(obj), "address", Value::Object(s_alloc0));
    let _ = vm.set_property(
        Value::Object(obj),
        "port",
        Value::Number(addr.port() as f64),
    );
    let family = if is_v4_family(&addr.ip()) {
        "IPv4"
    } else {
        "IPv6"
    };
    let s_alloc0 = vm.alloc_string(family.to_owned());
    let _ = vm.set_property(Value::Object(obj), "family", Value::Object(s_alloc0));
    Ok(Value::Object(obj))
}

/// `socket.connect(port[, address][, callback])`：设置默认发送目标。
fn dgram_connect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let mut port = 0u16;
    let mut address = String::new();
    let mut cb: Option<Value> = None;
    for a in args {
        if is_function(vm, *a) {
            cb = Some(*a);
        } else if let Some(n) = a.as_number {
            port = *n as u16;
        } else if matches!(a, Value::Object(_)) {
            address = vm.format_value(*a);
        }
    }
    let resolved = (address.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut i| i.next());
    if let Some(dest) = resolved {
        with_dgram(|s| {
            if let Some((_, state)) = s.sockets.iter_mut().find(|(sid, _)| *sid == r.0) {
                state.connected_dest = Some(dest);
            }
        });
        if let Some(cb) = cb {
            if is_function(vm, cb) {
                vm.invoke_callable(cb, Value::Undefined, &[Value::Null])?;
            }
        }
    } else if let Some(cb) = cb {
        if is_function(vm, cb) {
            let err = vm.alloc_error_instance("invalid address");
            vm.invoke_callable(cb, Value::Undefined, &[Value::Object(err)])?;
        }
    }
    Ok(Value::Object(r))
}

/// `socket.disconnect()`：清除默认发送目标。
fn dgram_disconnect(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    with_dgram(|s| {
        if let Some((_, state)) = s.sockets.iter_mut().find(|(sid, _)| *sid == r.0) {
            state.connected_dest = None;
        }
    });
    Ok(receiver)
}

/// 兼容性 no-op（setBroadcast 等），返回自身。
fn dgram_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

// --- 事件源泵 ---------------------------------------------------------------

/// dgram 事件源泵：派发待办（listening 等）→ 接收轮询（'message' /
/// 错误时 'close'）。全部 socket 关闭且队列排空时自动注销事件源。
fn dgram_pump(vm: &mut Vm) -> Result<bool, VmError> {
    let idle = with_dgram(|s| s.sockets.is_empty() && s.pending.is_empty());
    if idle {
        vm.deactivate_event_source("dgram");
        return Ok(false);
    }
    let mut progressed = false;

    // 1. 派发待办（bind 的 listening + 回调序列）。
    loop {
        let action = with_dgram(|s| s.pending.pop_front());
        let Some(action) = action else {
            break;
        };
        progressed = true;
        match action {
            DgramAction::Emit {
                target,
                event,
                args,
            } => {
                emit_dgram_event(vm, target, &event, &args)?;
            }
            DgramAction::Call { cb, args } => {
                vm.invoke_callable(cb, Value::Undefined, &args)?;
            }
        }
    }

    // 2. 接收轮询：数据 → 'message'（Buffer + rinfo）；错误 → 'close'。
    let socket_ids: Vec<u32> = with_dgram(|s| s.sockets.iter().map(|(id, _)| *id).collect());
    for sock_id in socket_ids {
        let recv = with_dgram(|s| {
            let (_, state) = s.sockets.iter_mut().find(|(sid, _)| *sid == sock_id)?;
            let sock = state.socket.as_mut()?;
            let mut buf = vec![0u8; RECV_CHUNK];
            match sock.recv_from(&mut buf) {
                Ok((n, peer)) => Some(Ok((buf[..n].to_vec(), peer))),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => None,
                Err(e) => Some(Err(e)),
            }
        });
        let Some(result) = recv else {
            continue;
        };
        progressed = true;
        match result {
            Ok((data, peer)) => {
                let len = data.len();
                let buf_obj = create_buffer_instance(vm, data);
                let obj_val = with_dgram(|s| {
                    s.sockets
                        .iter()
                        .find(|(sid, _)| *sid == sock_id)
                        .map(|(_, st)| st.obj)
                });
                if let Some(obj) = obj_val {
                    // rinfo：{address, port, family, size}（family 对齐 Go 大写形态）。
                    let rinfo = vm.alloc_ordinary();
                    let s_alloc0 = vm.alloc_string(peer.ip().to_string());
                    let _ =
                        vm.set_property(Value::Object(rinfo), "address", Value::Object(s_alloc0));
                    let _ = vm.set_property(
                        Value::Object(rinfo),
                        "port",
                        Value::Number(peer.port() as f64),
                    );
                    let family = if is_v4_family(&peer.ip()) {
                        "IPv4"
                    } else {
                        "IPv6"
                    };
                    let s_alloc0 = vm.alloc_string(family.to_owned());
                    let _ =
                        vm.set_property(Value::Object(rinfo), "family", Value::Object(s_alloc0));
                    let _ =
                        vm.set_property(Value::Object(rinfo), "size", Value::Number(len as f64));
                    with_dgram(|s| {
                        s.pending.push_back(DgramAction::Emit {
                            target: obj,
                            event: "message".to_owned(),
                            args: vec![Value::Object(buf_obj), Value::Object(rinfo)],
                        });
                    });
                }
            }
            Err(_) => {
                // 对齐 Go：接收错误且未显式关闭 → 移除并异步派发 'close'。
                let obj_val = with_dgram(|s| {
                    let obj = s
                        .sockets
                        .iter()
                        .find(|(sid, _)| *sid == sock_id)
                        .map(|(_, st)| st.obj);
                    s.sockets.retain(|(sid, _)| *sid != sock_id);
                    obj
                });
                if let Some(obj) = obj_val {
                    with_dgram(|s| {
                        s.pending.push_back(DgramAction::Emit {
                            target: obj,
                            event: "close".to_owned(),
                            args: Vec::new(),
                        });
                    });
                }
            }
        }
    }

    // 3. 派发 message / close。
    loop {
        let action = with_dgram(|s| s.pending.pop_front());
        let Some(action) = action else {
            break;
        };
        match action {
            DgramAction::Emit {
                target,
                event,
                args,
            } => {
                emit_dgram_event(vm, target, &event, &args)?;
            }
            DgramAction::Call { cb, args } => {
                vm.invoke_callable(cb, Value::Undefined, &args)?;
            }
        }
    }
    Ok(progressed)
}

/// IPv4 家族判定（IPv4 与 IPv4 映射地址均归 IPv4，对齐 Go To4 语义）。
fn is_v4_family(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(_) => true,
        IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some(),
    }
}

/// GC 根快照：dgram 侧表实例对象、监听器与待派发事件/回调。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    DGRAM_SHARED.with(|g| {
        let binding = g.borrow();
        let Some(shared) = binding.as_ref() else {
            return;
        };
        for (_, s) in &shared.sockets {
            out.push(s.obj);
            for items in s.listeners.values() {
                for l in items {
                    out.push(l.cb);
                }
            }
        }
        for action in &shared.pending {
            match action {
                DgramAction::Emit { args, .. } => {
                    for v in args {
                        out.push(*v);
                    }
                }
                DgramAction::Call { cb, args } => {
                    out.push(*cb);
                    for v in args {
                        out.push(*v);
                    }
                }
            }
        }
    });
}
