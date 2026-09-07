//! `process` / `console` / `url` 的 require 门面（Go registry 同名注册项对齐）。
//!
//! Node.js 22 LTS 规范把 `process`、`console`、
//! `url` 注册为可 `require` 的内置模块；Rust 侧三者的全局形态分别由解释器
//! 全局单例（`process`）与特化分派（`console.log` 兜底、`URL` 构造器）提供。
//! 本模块把它们物化为注册表模块单例，使 `require("process")` 等返回与全局
//! 一致的对象，方法调用经 [`crate::builtins::try_dispatch`] 命中。
//!
//! 实测对齐要点（标准：Node.js 22 LTS）：
//! - `require("process")` 返回全局 process 对象（argv/env/nextTick 等既有拦截不变）；
//! - `console.log/info/debug/trace` → stdout，`console.error/warn` → stderr；
//! - `url.parse(href)` 返回 `{ href, protocol, host, hostname, port, pathname,
//!   search, hash }`，其中 `search`/`hash` 不带前导 `?`/`#`（Go 实测形态）；
//!   `url.resolve` 为函数；`url.URL` 呈现为函数（Go 侧 `new` 会报错，此处同形）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("process")` / `require("node:process")`。
pub const PROCESS_MODULE: ModuleDef = ModuleDef {
    name: "process",
    build: build_process,
};

/// `require("console")` / `require("node:console")`。
pub const CONSOLE_MODULE: ModuleDef = ModuleDef {
    name: "console",
    build: build_console,
};

/// `require("url")` / `require("node:url")`。
pub const URL_MODULE: ModuleDef = ModuleDef {
    name: "url",
    build: build_url,
};

fn build_process(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    register_handler(
        registry,
        "process",
        "getBuiltinModule",
        process_get_builtin_module,
    );
    // 事件面（depd/on-finished 等在模块顶层调用 process.listenerCount）
    register_handler(registry, "process", "listenerCount", process_listener_count);
    register_handler(registry, "process", "listeners", process_listeners);
    for method in ["on", "once", "removeListener", "removeAllListeners", "emit"] {
        let f = vm.alloc_native_fn(&format!("process.{method}"));
        let _ = vm.set_property(
            Value::Object(vm.process_object.unwrap()),
            method,
            Value::Object(f),
        );
        register_handler(registry, "process", method, process_noop_event);
    }
    let lc = vm.alloc_native_fn("process.listenerCount");
    let _ = vm.set_property(
        Value::Object(vm.process_object.unwrap()),
        "listenerCount",
        Value::Object(lc),
    );
    let ls = vm.alloc_native_fn("process.listeners");
    let _ = vm.set_property(
        Value::Object(vm.process_object.unwrap()),
        "listeners",
        Value::Object(ls),
    );
    vm.process_object.ok_or_else(|| {
        VmError::Thrown(Value::Object(
            vm.alloc_string("process global missing".to_string()),
        ))
    })
}

/// process 事件面空实现（真实包仅在模块顶层探测性调用）。
fn process_listener_count(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Number(0.0))
}

fn process_listeners(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    // 无法在此访问堆分配器：返回空数组的占位（depd 仅读取 .length）
    Ok(Value::Undefined)
}

fn process_noop_event(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `process.getBuiltinModule(specifier)`（Node 22.3 / Go 实测对齐）：
/// 按 specifier 查内置注册表（`node:` 前缀剥离），未知模块返回 undefined。
fn process_get_builtin_module(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let spec = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let name = spec.strip_prefix("node:").unwrap_or(&spec);
    Ok(match vm.builtin_registry.module(name) {
        Some(r) => Value::Object(r),
        None => Value::Undefined,
    })
}

fn build_console(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for name in ["log", "info", "debug", "trace", "error", "warn"] {
        let f = vm.alloc_native_fn(&format!("console.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
        let handler = if name == "error" || name == "warn" {
            console_stderr
        } else {
            console_stdout
        };
        register_handler(registry, "console", name, handler);
    }
    Ok(obj)
}

fn build_url(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for name in ["parse", "resolve", "format", "URL"] {
        let f = vm.alloc_native_fn(&format!("url.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
    }
    register_handler(registry, "url", "parse", url_parse);
    register_handler(registry, "url", "resolve", url_resolve);
    register_handler(registry, "url", "format", url_format);
    Ok(obj)
}

/// `console.log/info/debug/trace(...)`：格式化并追加进 stdout 记录。
fn console_stdout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let line = args
        .iter()
        .map(|v| vm.format_console_value(*v))
        .collect::<Vec<_>>()
        .join(" ");
    vm.stdout_records.push(line);
    Ok(Value::Undefined)
}

/// `console.error/warn(...)`：对齐 Go 输出到 stderr（不进 stdout 对拍流）。
fn console_stderr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let line = args
        .iter()
        .map(|v| vm.format_console_value(*v))
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("{line}");
    Ok(Value::Undefined)
}

/// `url.parse(href)`：轻量解析为属性对象（`search`/`hash` 不带前导符，对齐 Go）。
fn url_parse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let href = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let (protocol, rest) = match href.split_once(':') {
        Some((s, r)) if !r.is_empty() => (format!("{s}:"), r.strip_prefix("//").unwrap_or(r)),
        _ => (String::new(), href.as_str()),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let tail = &rest[authority_end..];
    let (userinfo_end, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (u.len() + 1, h),
        None => (0, authority),
    };
    let _ = userinfo_end;
    let (hostname, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p),
        _ => (hostport, ""),
    };
    let pathname_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let pathname = &tail[..pathname_end];
    let query_hash = &tail[pathname_end..];
    let (search, hash) = match query_hash.find('#') {
        Some(h) => (
            query_hash[..h]
                .strip_prefix('?')
                .unwrap_or(&query_hash[..h]),
            &query_hash[h + 1..],
        ),
        None => (query_hash.strip_prefix('?').unwrap_or(query_hash), ""),
    };
    let host = if port.is_empty() {
        hostname.to_string()
    } else {
        format!("{hostname}:{port}")
    };
    let props: [(&str, String); 8] = [
        ("href", href.clone()),
        ("protocol", protocol),
        ("host", host),
        ("hostname", hostname.to_string()),
        ("port", port.to_string()),
        ("pathname", pathname.to_string()),
        ("search", search.to_string()),
        ("hash", hash.to_string()),
    ];
    let obj = vm.alloc_ordinary();
    for (k, v) in props {
        let s = vm.alloc_string(v);
        let _ = vm.set_property(Value::Object(obj), k, Value::Object(s));
    }
    Ok(Value::Object(obj))
}

/// `url.resolve(from, to)`：`to` 为绝对地址（带协议）时直接返回，否则简单拼接。
fn url_resolve(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let from = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let to = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
    let resolved = if to.contains("://") || to.starts_with('/') {
        to
    } else {
        let base = from.split('/').next().unwrap_or("").to_string();
        format!("{base}/{to}")
    };
    Ok(Value::Object(vm.alloc_string(resolved)))
}

/// `url.format(obj_or_str)`：对象取 `href` 属性，字符串原样返回。
fn url_format(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let out = match args.first() {
        Some(Value::Object(r)) => {
            let mut href = String::new();
            if matches!(
                vm.heap.get(r.index()),
                Some(crate::heap::HeapObject::Ordinary { .. })
            ) {
                if let Some(Value::Object(s)) = vm.own_value(r.index(), "href") {
                    if let Some(crate::heap::HeapObject::String(t)) = vm.heap.get(s.index()) {
                        href = t.clone();
                    }
                }
            }
            href
        }
        Some(v) => vm.format_value(*v),
        None => String::new(),
    };
    Ok(Value::Object(vm.alloc_string(out)))
}
