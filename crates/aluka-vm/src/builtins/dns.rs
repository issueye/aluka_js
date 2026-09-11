//! `node:dns` 内置模块（Phase 5）：域名解析（callback 风格）。
//!
//! 与 Node.js 22 LTS 标准（`nodenet/dns.go`）对齐：
//! - `lookup(hostname[, options], callback)` → `cb(err, address, family)`；
//! - `resolve(hostname[, rrtype], callback)` 及 `resolve4`/`resolve6`/
//!   `resolveAny`/`resolveCaa`/`resolveCname`/`resolveMx`/`resolveNaptr`/
//!   `resolveNs`/`resolvePtr`/`resolveSoa`/`resolveSrv`/`resolveTlsa`/`resolveTxt`；
//! - `lookupService(address, port, callback)`、`reverse(ip, callback)`；
//! - `getServers`/`setServers`、`setDefaultResultOrder`/`getDefaultResultOrder`
//!   （与 `node:dns/promises` 共享顺序状态）、`Resolver` 类、错误码常量；
//! - `promises` 属性：与 `node:dns/promises` 模块单例同一对象（Node 恒等式）。
//!
//! 解析底座与错误形态复用 [`crate::builtins::dns_promises`] 的共享实现；
//! 异步时序经 dns 事件源泵派发（回调晚于同步代码块，对齐 Go PostTask）。
//! 已知限制：std 无递归 DNS / 反向 PTR，相关记录类型按 Go 对确定性输入
//! （localhost）的实测形态返回空结果；`reverse`/`lookupService` 以
//! 「地址可解析性」表达 ENOTFOUND 形态。

use crate::builtins::dns_promises::{
    self, DEFAULT_RESULT_ORDER, dns_resolve_by_type, dns_resolve_ip, enqueue_dns,
    get_or_build_promises, is_function, is_plain_object, lookup_host_addrs, make_dns_error,
    port_service_name,
};
use crate::builtins::dns_resolver::{
    self, arpa_name, needs_wire, query_meta, reverse_needs_wire, rrtype_uses_system,
    start_callback_resolve, start_callback_reverse,
};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::net::IpAddr;

/// `require("dns")` / `require("node:dns")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef { name: "dns", build };

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    dns_promises::register_dns_constants(vm, obj)?;

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
        let fn_ref = vm.alloc_native_fn(&format!("dns.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    let resolver_ctor = vm.alloc_native_fn("dns.Resolver");
    set_module_prop(vm, obj, "Resolver", Value::Object(resolver_ctor))?;
    // Node 恒等式：dns.promises === require("node:dns/promises")。
    let promises = get_or_build_promises(vm, registry)?;
    set_module_prop(vm, obj, "promises", Value::Object(promises))?;

    register_handler(registry, "dns", "lookup", dns_lookup);
    register_handler(registry, "dns", "lookupService", dns_lookup_service);
    register_handler(registry, "dns", "resolve", dns_resolve);
    register_handler(registry, "dns", "resolve4", dns_resolve4);
    register_handler(registry, "dns", "resolve6", dns_resolve6);
    register_handler(registry, "dns", "resolveAny", dns_resolve_any);
    register_handler(registry, "dns", "resolveCaa", dns_resolve_caa);
    register_handler(registry, "dns", "resolveCname", dns_resolve_cname);
    register_handler(registry, "dns", "resolveMx", dns_resolve_mx);
    register_handler(registry, "dns", "resolveNaptr", dns_resolve_naptr);
    register_handler(registry, "dns", "resolveNs", dns_resolve_ns);
    register_handler(registry, "dns", "resolvePtr", dns_resolve_ptr);
    register_handler(registry, "dns", "resolveSoa", dns_resolve_soa);
    register_handler(registry, "dns", "resolveSrv", dns_resolve_srv);
    register_handler(registry, "dns", "resolveTlsa", dns_resolve_tlsa);
    register_handler(registry, "dns", "resolveTxt", dns_resolve_txt);
    register_handler(registry, "dns", "reverse", dns_reverse);
    register_handler(registry, "dns", "getServers", dns_get_servers);
    register_handler(registry, "dns", "setServers", dns_set_servers);
    register_handler(
        registry,
        "dns",
        "setDefaultResultOrder",
        dns_set_default_result_order,
    );
    register_handler(
        registry,
        "dns",
        "getDefaultResultOrder",
        dns_get_default_result_order,
    );
    register_handler(registry, "dns", "Resolver", dns_resolver_ctor);

    // Resolver 实例命名空间（callback 风格，与模块共享解析实现）。
    for (method, handler) in [
        ("resolve", dns_resolve as crate::builtins::BuiltinHandler),
        ("resolve4", dns_resolve4),
        ("resolve6", dns_resolve6),
        ("resolveAny", dns_resolve_any),
        ("resolveCaa", dns_resolve_caa),
        ("resolveCname", dns_resolve_cname),
        ("resolveMx", dns_resolve_mx),
        ("resolveNaptr", dns_resolve_naptr),
        ("resolveNs", dns_resolve_ns),
        ("resolvePtr", dns_resolve_ptr),
        ("resolveSoa", dns_resolve_soa),
        ("resolveSrv", dns_resolve_srv),
        ("resolveTlsa", dns_resolve_tlsa),
        ("resolveTxt", dns_resolve_txt),
        ("reverse", dns_reverse),
        ("getServers", dns_get_servers),
        ("setServers", dns_set_servers),
        ("cancel", resolver_cancel),
    ] {
        register_handler(registry, "dns:resolver", method, handler);
    }

    Ok(obj)
}

/// `dns.lookup(hostname[, options], callback)` → `cb(err, address, family)`。
fn dns_lookup(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        let err = vm.alloc_error_instance("dns.lookup: requires hostname and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    }
    let hostname = vm.format_value(args[0]);
    let mut cb = args[1];
    // (hostname, options, callback) 形式。
    if is_plain_object(vm, args[1]) && args.len() > 2 && is_function(vm, args[2]) {
        cb = args[2];
    }
    let call = match lookup_host_addrs(&hostname) {
        Some(addrs) if !addrs.is_empty() => {
            let family = dns_promises::ip_family(&addrs[0]);
            vec![
                Value::Null,
                Value::Object(vm.alloc_string(addrs[0].clone())),
                Value::Number(family),
            ]
        }
        _ => {
            // 对齐 Go asyncLookup：失败回调 (ENOTFOUND 错误对象, null)。
            vec![make_dns_error(vm, "ENOTFOUND", &hostname), Value::Null]
        }
    };
    enqueue_dns(vm, cb, call);
    Ok(Value::Undefined)
}

/// 解析 resolve 系列参数（对齐 Go parseResolveArgs：hostname[, rrtype], callback）。
fn parse_resolve_args(vm: &mut Vm, args: &[Value]) -> Option<(String, String, Value)> {
    if args.len() < 2 {
        return None;
    }
    let hostname = vm.format_value(args[0]);
    let mut rrtype = "A".to_owned();
    let mut cb = args[1];
    if !is_function(vm, args[1]) && args.len() > 2 && is_function(vm, args[2]) {
        rrtype = vm.format_value(args[1]);
        cb = args[2];
    }
    Some((hostname, rrtype, cb))
}

/// `dns.resolve(hostname[, rrtype], callback)`。
fn dns_resolve(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some((hostname, rrtype, cb)) = parse_resolve_args(vm, args) else {
        let err = vm.alloc_error_instance("dns.resolve: requires hostname and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    // A/AAAA/ANY 与本地单标签名走系统解析（既有确定性形态）；其余真实报文查询。
    if rrtype_uses_system(&rrtype) || !needs_wire(&hostname) {
        let result = dns_resolve_by_type(vm, &hostname, &rrtype);
        enqueue_dns(vm, cb, vec![Value::Null, result]);
        return Ok(Value::Undefined);
    }
    if let Some((shape, syscall)) = query_meta(&rrtype) {
        if start_callback_resolve(vm, &hostname, &rrtype, syscall, shape, cb) {
            return Ok(Value::Undefined);
        }
    }
    // 未知 rrtype 或后台不可用：回落旧确定性形态。
    let result = dns_resolve_by_type(vm, &hostname, &rrtype);
    enqueue_dns(vm, cb, vec![Value::Null, result]);
    Ok(Value::Undefined)
}

/// `dns.resolve4(hostname, callback)`。
fn dns_resolve4(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_ip_cb(vm, args, 4)
}

/// `dns.resolve6(hostname, callback)`。
fn dns_resolve6(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_ip_cb(vm, args, 6)
}

/// resolve4/6 共用骨架。
fn dns_resolve_ip_cb(vm: &mut Vm, args: &[Value], version: u8) -> Result<Value, VmError> {
    let Some((hostname, _rrtype, cb)) = parse_resolve_args(vm, args) else {
        let err = vm.alloc_error_instance("dns.resolve: requires hostname and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let result = dns_resolve_ip(vm, &hostname, version);
    enqueue_dns(vm, cb, vec![Value::Null, result]);
    Ok(Value::Undefined)
}

/// 记录类 resolve 统一骨架：本地单标签名保持旧确定性空数组；
/// 非本地名走真实报文查询（未知 rrtype 或后台不可用回落空数组）。
fn dns_resolve_empty(
    vm: &mut Vm,
    args: &[Value],
    label: &str,
    rrtype: &str,
) -> Result<Value, VmError> {
    let Some((hostname, _rrtype, cb)) = parse_resolve_args(vm, args) else {
        let err = vm.alloc_error_instance(&format!("dns.{label}: requires hostname and callback"));
        return Err(VmError::Thrown(Value::Object(err)));
    };
    if !needs_wire(&hostname) {
        let result = Value::Object(vm.alloc_array(Vec::new()));
        enqueue_dns(vm, cb, vec![Value::Null, result]);
        return Ok(Value::Undefined);
    }
    if let Some((shape, syscall)) = query_meta(rrtype) {
        if start_callback_resolve(vm, &hostname, rrtype, syscall, shape, cb) {
            return Ok(Value::Undefined);
        }
    }
    let result = Value::Object(vm.alloc_array(Vec::new()));
    enqueue_dns(vm, cb, vec![Value::Null, result]);
    Ok(Value::Undefined)
}

/// `resolveAny`（真实解析：`{type, address}` 对象数组）。
fn dns_resolve_any(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some((hostname, _rrtype, cb)) = parse_resolve_args(vm, args) else {
        let err = vm.alloc_error_instance("dns.resolveAny: requires hostname and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let result = dns_promises::dns_resolve_any(vm, &hostname);
    enqueue_dns(vm, cb, vec![Value::Null, result]);
    Ok(Value::Undefined)
}

/// `resolveCaa`。
fn dns_resolve_caa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveCaa", "CAA")
}

/// `resolveCname`。
fn dns_resolve_cname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveCname", "CNAME")
}

/// `resolveMx`。
fn dns_resolve_mx(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveMx", "MX")
}

/// `resolveNaptr`。
fn dns_resolve_naptr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveNaptr", "NAPTR")
}

/// `resolveNs`。
fn dns_resolve_ns(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveNs", "NS")
}

/// `resolvePtr`。
fn dns_resolve_ptr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolvePtr", "PTR")
}

/// `resolveSoa`：本地名 → 空对象（旧确定性形态）；非本地名 → SOA 报文查询
/// （无记录亦兑现空对象，Node 语义为 err/对象视域而定；离线保持空对象）。
fn dns_resolve_soa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some((hostname, _rrtype, cb)) = parse_resolve_args(vm, args) else {
        let err = vm.alloc_error_instance("dns.resolveSoa: requires hostname and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    if !needs_wire(&hostname) {
        let result = Value::Object(vm.alloc_ordinary());
        enqueue_dns(vm, cb, vec![Value::Null, result]);
        return Ok(Value::Undefined);
    }
    if let Some((shape, syscall)) = query_meta("SOA") {
        if start_callback_resolve(vm, &hostname, "SOA", syscall, shape, cb) {
            return Ok(Value::Undefined);
        }
    }
    let result = Value::Object(vm.alloc_ordinary());
    enqueue_dns(vm, cb, vec![Value::Null, result]);
    Ok(Value::Undefined)
}

/// `resolveSrv`。
fn dns_resolve_srv(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveSrv", "SRV")
}

/// `resolveTlsa`。
fn dns_resolve_tlsa(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveTlsa", "TLSA")
}

/// `resolveTxt`。
fn dns_resolve_txt(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    dns_resolve_empty(vm, args, "resolveTxt", "TXT")
}

/// `dns.lookupService(address, port, callback)` → `cb(err, {hostname, service})`。
fn dns_lookup_service(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 3 {
        let err = vm.alloc_error_instance("dns.lookupService: requires address, port and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    }
    let address = vm.format_value(args[0]);
    let port = vm.format_value(args[1]);
    let cb = args[2];
    let call = if address.parse::<IpAddr>().is_ok() {
        // 对齐 Go 形态：hostname 来自反向 PTR（std 不可得，置空串），
        // service 走端口映射（与 Go 一致）。
        let o = vm.alloc_ordinary();
        let s_alloc0 = vm.alloc_string(String::new());
        let _ = vm.set_property(Value::Object(o), "hostname", Value::Object(s_alloc0));
        let s_alloc0 = vm.alloc_string(port_service_name(&port));
        let _ = vm.set_property(Value::Object(o), "service", Value::Object(s_alloc0));
        vec![Value::Null, Value::Object(o)]
    } else {
        // 对齐 Go asyncResolve 错误路径：hostname 为空串（errHostname 恒 ""）。
        vec![make_dns_error(vm, "ENOTFOUND", ""), Value::Null]
    };
    enqueue_dns(vm, cb, call);
    Ok(Value::Undefined)
}

/// `dns.reverse(ip, callback)`：合法且非回环 IP 走真实 PTR 报文查询；
/// 回环/非法输入保持旧确定性形态（空数组 / ENOTFOUND）。
fn dns_reverse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        let err = vm.alloc_error_instance("dns.reverse: requires ip and callback");
        return Err(VmError::Thrown(Value::Object(err)));
    }
    let ip = vm.format_value(args[0]);
    let cb = args[1];
    if ip.parse::<IpAddr>().is_ok() && reverse_needs_wire(&ip) {
        if let Some(qname) = arpa_name(&ip) {
            if start_callback_reverse(vm, &ip, &qname, cb) {
                return Ok(Value::Undefined);
            }
        }
    }
    let call = if ip.parse::<IpAddr>().is_ok() {
        vec![Value::Null, Value::Object(vm.alloc_array(Vec::new()))]
    } else {
        vec![make_dns_error(vm, "ENOTFOUND", ""), Value::Null]
    };
    enqueue_dns(vm, cb, call);
    Ok(Value::Undefined)
}

/// `dns.getServers()`：进程内列表（初始空数组，`setServers` 后回读设定值）。
fn dns_get_servers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let servers = dns_resolver::get_servers();
    let vals: Vec<Value> = servers
        .into_iter()
        .map(|s| Value::Object(vm.alloc_string(s)))
        .collect();
    Ok(Value::Object(vm.alloc_array(vals)))
}

/// `dns.setServers(servers)`：记录进程内列表（后续报文查询生效）。
fn dns_set_servers(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(r) = args.first().copied().as_object {
        if let Some(HeapObject::Array { elements, .. }) = vm.heap.get(r.0 as usize) {
            let servers: Vec<String> = elements.iter().map(|v| vm.format_value(*v)).collect();
            dns_resolver::set_servers(servers);
        }
    }
    Ok(Value::Undefined)
}

/// `dns.setDefaultResultOrder(order)`（与 dns/promises 共享状态）。
fn dns_set_default_result_order(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(v) = args.first() {
        DEFAULT_RESULT_ORDER.with(|g| *g.borrow_mut() = Some(vm.format_value(*v)));
    }
    Ok(Value::Undefined)
}

/// `dns.getDefaultResultOrder()`。
fn dns_get_default_result_order(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let order = DEFAULT_RESULT_ORDER
        .with(|g| g.borrow().clone())
        .unwrap_or_else(|| "verbatim".to_owned());
    Ok(Value::Object(vm.alloc_string(order)))
}

/// callback 版 Resolver 实例暴露的方法（对齐 Go resolveMethods 列表）。
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
    "cancel",
    "getServers",
    "setServers",
];

/// `new dns.Resolver()`：callback 风格实例（方法共享模块实现，实参驱动）。
fn dns_resolver_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("dns:resolver".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in RESOLVER_METHODS {
        let fn_ref = vm.alloc_native_fn(&format!("dns:resolver.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    Ok(Value::Object(obj))
}

/// `resolver.cancel()`：no-op（对齐 Go）。
fn resolver_cancel(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// 编译期锚定：dns 队列类型与 promises 模块共享（防漂移）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::dns_promises::DNS_PENDING;

    #[test]
    fn resolver_surface_matches_go_list() {
        assert!(RESOLVER_METHODS.contains(&"reverse"));
        assert!(RESOLVER_METHODS.contains(&"cancel"));
        assert_eq!(RESOLVER_METHODS.len(), 18);
    }

    #[test]
    fn dns_queue_starts_empty_per_process() {
        // 共享队列在进程生命周期内惰性创建；此处仅验证可安全加锁。
        let len = DNS_PENDING.with(|g| g.borrow().len());
        assert_eq!(len, 0);
    }
}
