//! `node:dns/promises` 内置模块（Phase 5）：Promise 版 DNS API 与共享解析逻辑。
//!
//! 与 Node.js 22 LTS 标准（`nodenet/dns_promises.go`）对齐：
//! - `lookup` → `Promise<{address, family}>`（成功路径与 Go 逐字对齐）；
//! - `resolve` / `resolve4` / `resolve6` / `resolveAny` / `resolveCname` /
//!   `resolveMx` / `resolveNs` / `resolvePtr` / `resolveSrv` / `resolveTxt` /
//!   `resolveCaa` / `resolveNaptr` / `resolveTlsa` / `resolveSoa` / `reverse`；
//! - `lookupService`、`getServers`/`setServers`、`setDefaultResultOrder`/
//!   `getDefaultResultOrder`（与 `node:dns` 共享同一顺序状态）、`Resolver` 类。
//!
//! 解析底座：`lookup`/`resolve`/`resolve4`/`resolve6`/`resolveAny` 走
//! `std::net::ToSocketAddrs` 真实解析（与 Go 同用系统 resolver，地址序一致）；
//! 其余记录类型（CNAME/MX/NS/PTR/SRV/TXT 等）std 不提供递归解析，按 Go
//! oracle 对确定性输入（localhost）的实测输出形态返回空数组/空对象。
//!
//! 实现说明：ENOTFOUND 路径已随引擎 rejection 语义落地改为真实 Promise 拒绝
//! （原注「本引擎无 reject 语义」已过时；lookup/lookupService 走 reject 解析器）
//! 以错误对象兑现（探针只覆盖成功路径）。
//!
//! 异步时序：解析同步完成后经 [`DNS_PENDING`] 队列由 `activate_event_source
//! ("dns", dns_pump)` 泵派发，保证回调 / 兑现晚于同步代码块（对齐 Go
//! goroutine + PostTask）。队列排空后自动注销事件源。

use crate::builtins::dns_resolver::{
    self, arpa_name, needs_wire, query_meta, reverse_needs_wire, rrtype_uses_system,
    start_promise_resolve, start_promise_reverse,
};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::net::{IpAddr, ToSocketAddrs};

/// DNS 异步待派发动作（Promise 兑现 / 回调调用）。
pub(crate) enum DnsAction {
    /// 以 `args` 调用回调（Promise resolver 也是可调用对象，兑现 Promise）。
    Call {
        /// 回调或 resolver。
        cb: Value,
        /// 实参。
        args: Vec<Value>,
    },
}

// dns 家族共享的异步派发队列（`node:dns` 与 `node:dns/promises` 共用）。
thread_local! {
    // DNS_PENDING：线程局部（堆句柄仅本线程 Vm 有效）。
    pub(crate) static DNS_PENDING: RefCell<std::collections::VecDeque<DnsAction>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

// `setDefaultResultOrder` 共享状态（Node 22 默认 verbatim；解析本身不重排，
// 对齐 Go 观测：设置后 lookup 结果仍按系统序返回）。
thread_local! {
    // DEFAULT_RESULT_ORDER：线程局部（同线程的 dns 与 dns/promises 共享）。
    pub(crate) static DEFAULT_RESULT_ORDER: RefCell<Option<String>> = const { RefCell::new(None) };
}

// dns/promises 单例缓存（`dns.promises === require("node:dns/promises")`）。
thread_local! {
    // PROMISES_OBJ：线程局部（缓存句柄仅本线程堆有效）。
    static PROMISES_OBJ: RefCell<Option<u32>> = const { RefCell::new(None) };
}

/// `require("dns/promises")` / `require("node:dns/promises")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "dns/promises",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    get_or_build_promises(vm, registry)
}

/// 取回（或首次构建）dns.promises 共享单例；`node:dns` 的 `promises` 属性
/// 与 `node:dns/promises` 模块单例是同一对象（对齐 Node 恒等式）。
pub(crate) fn get_or_build_promises(
    vm: &mut Vm,
    registry: &mut BuiltinRegistry,
) -> Result<ObjectRef, VmError> {
    let cached = PROMISES_OBJ.with(|g| *g.borrow());
    if let Some(id) = cached {
        // 同一堆内的缓存句柄直接复用（跨 VM 实例句柄失效则重建）。
        if (id as usize) < vm.heap.len() {
            return Ok(ObjectRef(id));
        }
    }
    let obj = build_promises_obj(vm, registry)?;
    PROMISES_OBJ.with(|g| *g.borrow_mut() = Some(obj.0));
    Ok(obj)
}

/// promises Resolver 实例暴露的方法（对齐 Go：不含 lookup/lookupService）。
const RESOLVER_METHODS: &[&str] = &[
    "resolve",
    "resolve4",
    "resolve6",
    "resolveAny",
    "resolveCaa",
    "resolveCname",
    "resolveMx",
    "resolveNaptr",
    "resolveNs",
    "resolvePtr",
    "resolveSoa",
    "resolveSrv",
    "resolveTlsa",
    "resolveTxt",
    "reverse",
    "getServers",
    "setServers",
    "cancel",
];

/// 构建 dns.promises 导出对象并登记分派处理器。
fn build_promises_obj(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    register_dns_constants(vm, obj)?;

    for method in [
        "lookup",
        "lookupService",
        "resolve",
        "resolve4",
        "resolve6",
        "resolveAny",
        "resolveCaa",
        "resolveCname",
        "resolveMx",
        "resolveNaptr",
        "resolveNs",
        "resolvePtr",
        "resolveSoa",
        "resolveSrv",
        "resolveTlsa",
        "resolveTxt",
        "reverse",
        "getServers",
        "setServers",
        "setDefaultResultOrder",
        "getDefaultResultOrder",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("dns/promises.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    let resolver_ctor = vm.alloc_native_fn("dns/promises.Resolver");
    set_module_prop(vm, obj, "Resolver", Value::Object(resolver_ctor))?;

    register_handler(registry, "dns/promises", "lookup", promises_lookup);
    register_handler(
        registry,
        "dns/promises",
        "lookupService",
        promises_lookup_service,
    );
    register_handler(registry, "dns/promises", "resolve", promises_resolve);
    register_handler(registry, "dns/promises", "resolve4", promises_resolve4);
    register_handler(registry, "dns/promises", "resolve6", promises_resolve6);
    register_handler(registry, "dns/promises", "resolveAny", promises_resolve_any);
    register_handler(registry, "dns/promises", "resolveCaa", promises_resolve_caa);
    register_handler(
        registry,
        "dns/promises",
        "resolveCname",
        promises_resolve_cname,
    );
    register_handler(registry, "dns/promises", "resolveMx", promises_resolve_mx);
    register_handler(
        registry,
        "dns/promises",
        "resolveNaptr",
        promises_resolve_naptr,
    );
    register_handler(registry, "dns/promises", "resolveNs", promises_resolve_ns);
    register_handler(registry, "dns/promises", "resolvePtr", promises_resolve_ptr);
    register_handler(registry, "dns/promises", "resolveSoa", promises_resolve_soa);
    register_handler(registry, "dns/promises", "resolveSrv", promises_resolve_srv);
    register_handler(
        registry,
        "dns/promises",
        "resolveTlsa",
        promises_resolve_tlsa,
    );
    register_handler(registry, "dns/promises", "resolveTxt", promises_resolve_txt);
    register_handler(registry, "dns/promises", "reverse", promises_reverse);
    register_handler(registry, "dns/promises", "getServers", promises_get_servers);
    register_handler(registry, "dns/promises", "setServers", promises_set_servers);
    register_handler(
        registry,
        "dns/promises",
        "setDefaultResultOrder",
        promises_set_default_result_order,
    );
    register_handler(
        registry,
        "dns/promises",
        "getDefaultResultOrder",
        promises_get_default_result_order,
    );
    register_handler(registry, "dns/promises", "Resolver", promises_resolver_ctor);

    // Resolver 实例命名空间：方法与 promises 模块共享实现（实参驱动）。
    for (method, handler) in [
        (
            "resolve",
            promises_resolve as crate::builtins::BuiltinHandler,
        ),
        ("resolve4", promises_resolve4),
        ("resolve6", promises_resolve6),
        ("resolveAny", promises_resolve_any),
        ("resolveCaa", promises_resolve_caa),
        ("resolveCname", promises_resolve_cname),
        ("resolveMx", promises_resolve_mx),
        ("resolveNaptr", promises_resolve_naptr),
        ("resolveNs", promises_resolve_ns),
        ("resolvePtr", promises_resolve_ptr),
        ("resolveSoa", promises_resolve_soa),
        ("resolveSrv", promises_resolve_srv),
        ("resolveTlsa", promises_resolve_tlsa),
        ("resolveTxt", promises_resolve_txt),
        ("reverse", promises_reverse),
        ("getServers", promises_get_servers),
        ("setServers", promises_set_servers),
        ("cancel", resolver_cancel),
    ] {
        register_handler(registry, "dns/promises:resolver", method, handler);
    }

    Ok(obj)
}

// --- 共享解析底座（node:dns 与 node:dns/promises 复用） ---------------------

/// `std::net::ToSocketAddrs` 真实解析；返回规范化地址串列表（保持系统序）。
pub(crate) fn lookup_host_addrs(hostname: &str) -> Option<Vec<String>> {
    match (hostname, 0u16).to_socket_addrs() {
        Ok(addrs) => Some(addrs.map(|a| a.ip().to_string()).collect()),
        Err(_) => None,
    }
}

/// 地址族判定：IPv4（含 IPv4 映射地址）→ 4，IPv6 → 6，非 IP → 0（对齐 Go ipFamily）。
pub(crate) fn ip_family(addr: &str) -> f64 {
    match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => 4.0,
        Ok(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_some() => 4.0,
        Ok(IpAddr::V6(_)) => 6.0,
        Err(_) => 0.0,
    }
}

/// 判断值是否为可调用对象。
pub(crate) fn is_function(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
    ))
}

/// 判断对象值是否为普通对象。
pub(crate) fn is_plain_object(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Ordinary { .. })
    ))
}

/// 构造带 code/errno/hostname 的 DNS 错误对象（对齐 Go makeDNSError）。
pub(crate) fn make_dns_error(vm: &mut Vm, code: &str, hostname: &str) -> Value {
    let err = vm.alloc_error_instance(&format!("{code} {hostname}"));
    let s_alloc0 = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "code", Value::Object(s_alloc0));
    let s_alloc0 = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "errno", Value::Object(s_alloc0));
    let s_alloc0 = vm.alloc_string(hostname.to_owned());
    let _ = vm.set_property(Value::Object(err), "hostname", Value::Object(s_alloc0));
    Value::Object(err)
}

/// 简化端口 → 服务名映射（对齐 Go portServiceName）。
pub(crate) fn port_service_name(port: &str) -> String {
    match port {
        "80" => "http".to_owned(),
        "443" => "https".to_owned(),
        "21" => "ftp".to_owned(),
        "25" => "smtp".to_owned(),
        "22" => "ssh".to_owned(),
        "53" => "domain".to_owned(),
        other => other.to_owned(),
    }
}

/// 按 IP 版本过滤解析结果（对齐 Go dnsResolveIP：永不报错，失败为空数组）。
pub(crate) fn dns_resolve_ip(vm: &mut Vm, hostname: &str, version: u8) -> Value {
    let mut vals: Vec<Value> = Vec::new();
    if let Some(addrs) = lookup_host_addrs(hostname) {
        for a in addrs {
            let family = ip_family(&a);
            if (version == 4 && family == 4.0) || (version == 6 && family == 6.0) {
                vals.push(Value::Object(vm.alloc_string(a)));
            }
        }
    }
    Value::Object(vm.alloc_array(vals))
}

/// resolveAny：全部地址以 `{type:"A", address}` 呈现（对齐 Go 近似实现，含 ::1）。
pub(crate) fn dns_resolve_any(vm: &mut Vm, hostname: &str) -> Value {
    let mut vals: Vec<Value> = Vec::new();
    if let Some(addrs) = lookup_host_addrs(hostname) {
        for a in addrs {
            let o = vm.alloc_ordinary();
            let s_alloc0 = vm.alloc_string("A".to_owned());
            let _ = vm.set_property(Value::Object(o), "type", Value::Object(s_alloc0));
            let s_alloc0 = vm.alloc_string(a);
            let _ = vm.set_property(Value::Object(o), "address", Value::Object(s_alloc0));
            vals.push(Value::Object(o));
        }
    }
    Value::Object(vm.alloc_array(vals))
}

/// 按记录类型解析（对齐 Go dnsResolveByType 的输出形态；std 无递归 DNS 的
/// 记录类型返回空数组，SOA 返回空对象——与 Go 对确定性输入的实测一致）。
pub(crate) fn dns_resolve_by_type(vm: &mut Vm, hostname: &str, rrtype: &str) -> Value {
    match rrtype {
        "A" => dns_resolve_ip(vm, hostname, 4),
        "AAAA" => dns_resolve_ip(vm, hostname, 6),
        "ANY" => dns_resolve_any(vm, hostname),
        "SOA" => Value::Object(vm.alloc_ordinary()),
        _ => Value::Object(vm.alloc_array(Vec::new())),
    }
}

/// 注册 DNS 错误码常量（Node 22 全集，字符串 errno，对齐 Go dnsErrorCodes）。
pub(crate) fn register_dns_constants(vm: &mut Vm, m: ObjectRef) -> Result<(), VmError> {
    for (key, val) in dns_error_codes() {
        let val_ref = Value::Object(vm.alloc_string(val.to_owned()));
        set_module_prop(vm, m, key, val_ref)?;
    }
    Ok(())
}

/// DNS 错误码常量表（按名称排序，保证注册确定性）。
pub(crate) fn dns_error_codes() -> Vec<(&'static str, &'static str)> {
    [
        ("ADDRGETNETWORKPARAMS", "EADDRGETNETWORKPARAMS"),
        ("BADFAMILY", "EBADFAMILY"),
        ("BADFLAGS", "EBADFLAGS"),
        ("BADHINTS", "EBADHINTS"),
        ("BADNAME", "EBADNAME"),
        ("BADQUERY", "EBADQUERY"),
        ("BADRESP", "EBADRESP"),
        ("BADSTR", "EBADSTR"),
        ("CANCELLED", "ECANCELLED"),
        ("CONNREFUSED", "ECONNREFUSED"),
        ("DESTRUCTION", "EDESTRUCTION"),
        ("EOF", "EOF"),
        ("FILE", "EFILE"),
        ("FORMERR", "EFORMERR"),
        ("LOADIPHLPAPI", "ELOADIPHLPAPI"),
        ("NODATA", "ENODATA"),
        ("NOMEM", "ENOMEM"),
        ("NONAME", "ENONAME"),
        ("NOTFOUND", "ENOTFOUND"),
        ("NOTIMP", "ENOTIMP"),
        ("NOTINITIALIZED", "ENOTINITIALIZED"),
        ("REFUSED", "EREFUSED"),
        ("SERVFAIL", "ESERVFAIL"),
        ("TIMEOUT", "ETIMEOUT"),
    ]
    .into_iter()
    .collect()
}

/// 入队 dns 异步动作并激活 dns 事件源。
pub(crate) fn enqueue_dns(vm: &mut Vm, cb: Value, args: Vec<Value>) {
    DNS_PENDING.with(|g| g.borrow_mut().push_back(DnsAction::Call { cb, args }));
    vm.activate_event_source("dns", dns_pump);
}

/// dns 事件源泵：排空异步派发队列，队列空时注销事件源。
fn dns_pump(vm: &mut Vm) -> Result<bool, VmError> {
    let mut progressed = false;
    loop {
        let action = DNS_PENDING.with(|g| g.borrow_mut().pop_front());
        let Some(DnsAction::Call { cb, args }) = action else {
            break;
        };
        vm.invoke_callable(cb, Value::Undefined, &args)?;
        progressed = true;
    }
    if !progressed {
        vm.deactivate_event_source("dns");
    }
    Ok(progressed)
}

// --- Promise 化方法 ---------------------------------------------------------

/// 分配 pending Promise 与其 resolver。
fn alloc_resolver(vm: &mut Vm) -> (ObjectRef, Value) {
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);
    (promise, Value::Object(resolver))
}

/// `dns.promises.lookup(hostname)` → `Promise<{address, family}>`。
fn promises_lookup(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance("dns.promises.lookup: requires hostname");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    match lookup_host_addrs(&hostname) {
        Some(addrs) if !addrs.is_empty() => {
            let o = vm.alloc_ordinary();
            let s_alloc0 = vm.alloc_string(addrs[0].clone());
            let _ = vm.set_property(Value::Object(o), "address", Value::Object(s_alloc0));
            let _ = vm.set_property(
                Value::Object(o),
                "family",
                Value::Number(ip_family(&addrs[0])),
            );
            enqueue_dns(vm, resolver, vec![Value::Object(o)]);
        }
        _ => {
            // ENOTFOUND：Promise 拒绝（Go 语义；引擎已支持 rejection）
            let reject = vm.alloc_promise_resolver(promise, false);
            let err = make_dns_error(vm, "ENOTFOUND", &hostname);
            enqueue_dns(vm, Value::Object(reject), vec![err]);
        }
    }
    Ok(Value::Object(promise))
}

/// `dns.promises.lookupService(address, port)` → `Promise<{hostname, service}>`。
fn promises_lookup_service(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (Some(address_arg), Some(port_arg)) = (args.first().copied(), args.get(1).copied()) else {
        let err = vm.alloc_error_instance("dns.promises.lookupService: requires address and port");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let address = vm.format_value(address_arg);
    let port = vm.format_value(port_arg);
    let (promise, resolver) = alloc_resolver(vm);
    if address.parse::<IpAddr>().is_ok() {
        // 反向 PTR 记录 std 不可得：hostname 置空串（服务名映射与 Go 一致）。
        let o = vm.alloc_ordinary();
        let s_alloc0 = vm.alloc_string(String::new());
        let _ = vm.set_property(Value::Object(o), "hostname", Value::Object(s_alloc0));
        let s_alloc0 = vm.alloc_string(port_service_name(&port));
        let _ = vm.set_property(Value::Object(o), "service", Value::Object(s_alloc0));
        enqueue_dns(vm, resolver, vec![Value::Object(o)]);
    } else {
        // ENOTFOUND：Promise 拒绝（Go 语义；引擎已支持 rejection）
        let reject = vm.alloc_promise_resolver(promise, false);
        let err = make_dns_error(vm, "ENOTFOUND", &address);
        enqueue_dns(vm, Value::Object(reject), vec![err]);
    }
    Ok(Value::Object(promise))
}

/// `dns.promises.resolve(hostname[, rrtype])`。
fn promises_resolve(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance("dns.promises.resolve: requires hostname");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let rrtype = args
        .get(1)
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "A".to_owned());
    let (promise, resolver) = alloc_resolver(vm);
    // A/AAAA/ANY 与本地单标签名走系统解析（既有确定性形态）；其余真实报文查询。
    if rrtype_uses_system(&rrtype) || !needs_wire(&hostname) {
        let result = dns_resolve_by_type(vm, &hostname, &rrtype);
        enqueue_dns(vm, resolver, vec![result]);
        return Ok(Value::Object(promise));
    }
    if let Some((shape, syscall)) = query_meta(&rrtype) {
        let reject = vm.alloc_promise_resolver(promise, false);
        if start_promise_resolve(
            vm,
            &hostname,
            &rrtype,
            syscall,
            shape,
            resolver,
            Value::Object(reject),
        ) {
            return Ok(Value::Object(promise));
        }
    }
    // 未知 rrtype 或后台不可用：回落旧确定性形态。
    let result = dns_resolve_by_type(vm, &hostname, &rrtype);
    enqueue_dns(vm, resolver, vec![result]);
    Ok(Value::Object(promise))
}

/// `dns.promises.resolve4(hostname)`。
fn promises_resolve4(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_resolve_ip(vm, args, "dns.promises.resolve4: requires hostname", 4)
}

/// `dns.promises.resolve6(hostname)`。
fn promises_resolve6(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_resolve_ip(vm, args, "dns.promises.resolve6: requires hostname", 6)
}

/// resolve4/6 共用骨架。
fn promises_resolve_ip(
    vm: &mut Vm,
    args: &[Value],
    missing_message: &str,
    version: u8,
) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance(missing_message);
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    let result = dns_resolve_ip(vm, &hostname, version);
    enqueue_dns(vm, resolver, vec![result]);
    Ok(Value::Object(promise))
}

/// `dns.promises.resolveAny(hostname)`。
fn promises_resolve_any(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance("dns.promises.resolveAny: requires hostname");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    let result = dns_resolve_any(vm, &hostname);
    enqueue_dns(vm, resolver, vec![result]);
    Ok(Value::Object(promise))
}

/// 记录类 resolve 统一骨架：本地单标签名保持旧确定性空数组；
/// 非本地名走真实报文查询（未知 rrtype 或后台不可用回落空数组）。
fn promises_empty_result(
    vm: &mut Vm,
    args: &[Value],
    label: &str,
    rrtype: &str,
) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance(&format!("dns.promises.{label}: requires hostname"));
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    if !needs_wire(&hostname) {
        let empty = Value::Object(vm.alloc_array(Vec::new()));
        enqueue_dns(vm, resolver, vec![empty]);
        return Ok(Value::Object(promise));
    }
    if let Some((shape, syscall)) = query_meta(rrtype) {
        let reject = vm.alloc_promise_resolver(promise, false);
        if start_promise_resolve(
            vm,
            &hostname,
            rrtype,
            syscall,
            shape,
            resolver,
            Value::Object(reject),
        ) {
            return Ok(Value::Object(promise));
        }
    }
    let empty = Value::Object(vm.alloc_array(Vec::new()));
    enqueue_dns(vm, resolver, vec![empty]);
    Ok(Value::Object(promise))
}

/// `resolveCaa`。
fn promises_resolve_caa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveCaa", "CAA")
}

/// `resolveCname`。
fn promises_resolve_cname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveCname", "CNAME")
}

/// `resolveMx`。
fn promises_resolve_mx(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveMx", "MX")
}

/// `resolveNaptr`。
fn promises_resolve_naptr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveNaptr", "NAPTR")
}

/// `resolveNs`。
fn promises_resolve_ns(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveNs", "NS")
}

/// `resolvePtr`。
fn promises_resolve_ptr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolvePtr", "PTR")
}

/// `resolveSrv`。
fn promises_resolve_srv(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveSrv", "SRV")
}

/// `resolveTlsa`。
fn promises_resolve_tlsa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveTlsa", "TLSA")
}

/// `resolveTxt`。
fn promises_resolve_txt(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    promises_empty_result(vm, args, "resolveTxt", "TXT")
}

/// `reverse`（Promise 版）：非回环合法 IP 走真实 PTR；回环/非法输入保持
/// 旧确定性形态（空数组）。
fn promises_reverse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance("dns.promises.reverse: requires ip");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let ip = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    if ip.parse::<IpAddr>().is_ok() && reverse_needs_wire(&ip) {
        if let Some(qname) = arpa_name(&ip) {
            let reject = vm.alloc_promise_resolver(promise, false);
            if start_promise_reverse(vm, &ip, &qname, resolver, Value::Object(reject)) {
                return Ok(Value::Object(promise));
            }
        }
    }
    let empty = Value::Object(vm.alloc_array(Vec::new()));
    enqueue_dns(vm, resolver, vec![empty]);
    Ok(Value::Object(promise))
}

/// `dns.promises.resolveSoa(hostname)`：本地名 → 空对象 Promise；
/// 非本地名走 SOA 报文查询（无记录亦兑现空对象，保持旧形态；出错则拒绝）。
fn promises_resolve_soa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first().copied() else {
        let err = vm.alloc_error_instance("dns.promises.resolveSoa: requires hostname");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let hostname = vm.format_value(arg);
    let (promise, resolver) = alloc_resolver(vm);
    if !needs_wire(&hostname) {
        let result = Value::Object(vm.alloc_ordinary());
        enqueue_dns(vm, resolver, vec![result]);
        return Ok(Value::Object(promise));
    }
    if let Some((shape, syscall)) = query_meta("SOA") {
        let reject = vm.alloc_promise_resolver(promise, false);
        if start_promise_resolve(
            vm,
            &hostname,
            "SOA",
            syscall,
            shape,
            resolver,
            Value::Object(reject),
        ) {
            return Ok(Value::Object(promise));
        }
    }
    let result = Value::Object(vm.alloc_ordinary());
    enqueue_dns(vm, resolver, vec![result]);
    Ok(Value::Object(promise))
}

/// `dns.promises.getServers()`：进程内列表（初始空数组，与 node:dns 共享）。
fn promises_get_servers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let servers = dns_resolver::get_servers();
    let vals: Vec<Value> = servers
        .into_iter()
        .map(|s| Value::Object(vm.alloc_string(s)))
        .collect();
    Ok(Value::Object(vm.alloc_array(vals)))
}

/// `dns.promises.setServers(servers)`：记录进程内列表（后续报文查询生效）。
fn promises_set_servers(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(Value::Object(r)) = args.first().copied() {
        if let Some(HeapObject::Array { elements, .. }) = vm.heap.get(r.0 as usize) {
            let servers: Vec<String> = elements.iter().map(|v| vm.format_value(*v)).collect();
            dns_resolver::set_servers(servers);
        }
    }
    Ok(Value::Undefined)
}

/// `dns.promises.setDefaultResultOrder(order)`（与 node:dns 共享状态）。
fn promises_set_default_result_order(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(v) = args.first() {
        DEFAULT_RESULT_ORDER.with(|g| *g.borrow_mut() = Some(vm.format_value(*v)));
    }
    Ok(Value::Undefined)
}

/// `dns.promises.getDefaultResultOrder()`。
fn promises_get_default_result_order(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let order = DEFAULT_RESULT_ORDER
        .with(|g| g.borrow().clone())
        .unwrap_or_else(|| "verbatim".to_owned());
    Ok(Value::Object(vm.alloc_string(order)))
}

/// `new dns.promises.Resolver()`：实例挂共享命名空间，方法与 promises 模块同实现。
fn promises_resolver_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("dns/promises:resolver".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in RESOLVER_METHODS {
        let fn_ref = vm.alloc_native_fn(&format!("dns/promises:resolver.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    Ok(Value::Object(obj))
}

/// `resolver.cancel()`：no-op（对齐 Go）。
fn resolver_cancel(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// 编译期锚定：常量表覆盖与 Go dnsErrorCodes 一致（25 项）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_error_codes_cover_go_set() {
        // Go dnsErrorCodes 全集 24 项（ADDRGETNETWORKPARAMS..TIMEOUT）。
        assert_eq!(dns_error_codes().len(), 24);
        assert!(dns_error_codes().contains(&("NODATA", "ENODATA")));
        assert!(dns_error_codes().contains(&("NOTFOUND", "ENOTFOUND")));
    }

    #[test]
    fn ip_family_matches_go_semantics() {
        assert_eq!(ip_family("127.0.0.1"), 4.0);
        assert_eq!(ip_family("::1"), 6.0);
        assert_eq!(ip_family("localhost"), 0.0);
    }

    #[test]
    fn port_service_name_maps_common_ports() {
        assert_eq!(port_service_name("80"), "http");
        assert_eq!(port_service_name("443"), "https");
        assert_eq!(port_service_name("8080"), "8080");
    }

    #[test]
    fn resolver_surface_excludes_lookup() {
        assert!(!RESOLVER_METHODS.contains(&"lookup"));
        assert!(!RESOLVER_METHODS.contains(&"lookupService"));
        assert!(RESOLVER_METHODS.contains(&"cancel"));
    }
}
