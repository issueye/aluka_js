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
//!   `url.resolve` 为函数；`url.URL` 呈现为函数（Go 侧 `new` 会报错，此处同形）；
//! - **`process` 事件面为真实事件器（M5.2）**：`on`/`addListener`/`once`/`off`/
//!   `removeListener`/`removeAllListeners`/`emit`/`listenerCount`/`listeners`
//!   按 Node 语义（`on` 返回 `process` 自身、`off === removeListener`、
//!   `addListener === on`、`emit` 返回是否有监听器、`once` 触发即自删、`listeners`
//!   返回副本）。监听器与 `_builtinNs` 实例事件器**共用存储**（GC 根同源），
//!   但一律以 `process` 单例句柄为键——方法经 `NativeFn` 名分派，
//!   `current_receiver()` 是方法函数而非实例。worker 侧 `'message'`/`'disconnect'`
//!   的派发见 `builtins::cluster`（含 Node channel ref 保活与桥接语义）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
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
    register_handler(registry, "process", "exit", process_exit);
    register_handler(registry, "process", "cwd", process_cwd);
    // 事件面（M5.2）：**真实事件器**（Node `process` 是 EventEmitter）——监听器存在
    // 按实例句柄登记的事件器表中（与 `_builtinNs` 实例共用存储，GC 根同源），
    // `on/once/off/...` 返回 `process` 自身。事件方法经 `NativeFn` 名分派，
    // `current_receiver()` 是方法函数而非实例，故一律以 `process` 单例句柄为键。
    //
    // 别名共用**同一个函数对象**（Node `addListener === on`、`off === removeListener`），
    // 故 `process.addListener(...)` 的分派名仍是 `process.on`（已登记）。
    let method_groups: [(&[&str], &str); 5] = [
        (&["on", "addListener"], "on"),
        (&["once"], "once"),
        (&["off", "removeListener"], "off"),
        (&["removeAllListeners"], "removeAllListeners"),
        (&["emit"], "emit"),
    ];
    for (names, canonical) in method_groups {
        let f = vm.alloc_native_fn(&format!("process.{canonical}"));
        for name in names {
            let _ = vm.set_property(
                Value::Object(vm.process_object.unwrap()),
                name,
                Value::Object(f),
            );
        }
    }
    for method in ["listenerCount", "listeners"] {
        let f = vm.alloc_native_fn(&format!("process.{method}"));
        let _ = vm.set_property(
            Value::Object(vm.process_object.unwrap()),
            method,
            Value::Object(f),
        );
    }
    register_handler(registry, "process", "on", process_event_on);
    register_handler(registry, "process", "addListener", process_event_on);
    register_handler(registry, "process", "once", process_event_once);
    register_handler(registry, "process", "off", process_event_off);
    register_handler(registry, "process", "removeListener", process_event_off);
    register_handler(
        registry,
        "process",
        "removeAllListeners",
        process_event_remove_all,
    );
    register_handler(registry, "process", "emit", process_event_emit);
    register_handler(
        registry,
        "process",
        "listenerCount",
        process_event_listener_count,
    );
    register_handler(registry, "process", "listeners", process_event_listeners);
    let cwd = vm.alloc_native_fn("process.cwd");
    let _ = vm.set_property(
        Value::Object(vm.process_object.unwrap()),
        "cwd",
        Value::Object(cwd),
    );
    let exit = vm.alloc_native_fn("process.exit");
    let _ = vm.set_property(
        Value::Object(vm.process_object.unwrap()),
        "exit",
        Value::Object(exit),
    );
    vm.process_object.ok_or_else(|| {
        VmError::Thrown(Value::Object(
            vm.alloc_string("process global missing".to_string()),
        ))
    })
}

/// `process.exit(code)`：立即终止（Node 语义，绕过 try/catch 直达宿主；
/// 退出码省略或非数字时按 0）。
pub(crate) fn process_exit(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let code = args
        .first()
        .map(|v| crate::ops::to_number(*v) as i32)
        .unwrap_or(0);
    Err(VmError::Exit(code))
}

/// `process.cwd()`：当前工作目录（express 路由/文件路径推理常用）。
pub(crate) fn process_cwd(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_owned());
    Ok(Value::Object(vm.alloc_string(cwd)))
}

// ---------------------------------------------------------------------------
// process 事件器（M5.2）：Node `process` 是 EventEmitter，事件面语义与
// `_builtinNs` 实例事件器一致，但监听器一律以 `process` 单例句柄为键
// （方法经 `NativeFn` 名分派，`current_receiver()` 是方法函数不是实例）。
// ---------------------------------------------------------------------------

/// `process` 单例句柄（事件器的存储键）。
fn process_handle(vm: &Vm) -> Option<u32> {
    vm.process_object.map(|r| r.0)
}

/// `process.on(event, cb)` / `addListener`：返回 `process` 自身（Node 语义）。
fn process_event_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 {
        let event = vm.to_property_key(args[0]);
        crate::builtins::child_process::proc_common::emitter_add(id, &event, args[1], false);
    }
    Ok(Value::Object(ObjectRef(id)))
}

/// `process.once(event, cb)`。
fn process_event_once(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 {
        let event = vm.to_property_key(args[0]);
        crate::builtins::child_process::proc_common::emitter_add(id, &event, args[1], true);
    }
    Ok(Value::Object(ObjectRef(id)))
}

/// `process.off(event, cb)` / `removeListener`。
fn process_event_off(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Undefined);
    };
    if args.len() >= 2 {
        let event = vm.to_property_key(args[0]);
        crate::builtins::child_process::proc_common::emitter_remove(id, &event, args[1]);
    }
    Ok(Value::Object(ObjectRef(id)))
}

/// `process.removeAllListeners([event])`。
fn process_event_remove_all(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Undefined);
    };
    let event = match args.first() {
        Some(v) if !matches!(*v, Value::Undefined) => Some(vm.to_property_key(*v)),
        _ => None,
    };
    crate::builtins::child_process::proc_common::emitter_remove_all(id, event.as_deref());
    Ok(Value::Object(ObjectRef(id)))
}

/// `process.emit(event, ...args)`：返回是否**曾**有监听器（Node 语义）；
/// `error` 事件无监听器时抛原值（与 `_builtinNs` 实例事件器一致）。
fn process_event_emit(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Boolean(false));
    };
    let Some(event_val) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let event = vm.to_property_key(event_val);
    let had = crate::builtins::child_process::proc_common::ns_listener_count(id, &event) > 0;
    let emit_args: Vec<Value> = args.iter().skip(1).copied().collect();
    crate::builtins::child_process::proc_common::ns_emit(
        vm,
        Value::Object(ObjectRef(id)),
        &event,
        &emit_args,
    )?;
    Ok(Value::Boolean(had))
}

/// `process.listenerCount(event)`。
fn process_event_listener_count(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Number(0.0));
    };
    let event = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let n = crate::builtins::child_process::proc_common::ns_listener_count(id, &event);
    Ok(Value::Number(n as f64))
}

/// `process.listeners(event)`：监听器数组的副本（Node 语义）。
fn process_event_listeners(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(id) = process_handle(vm) else {
        return Ok(Value::Undefined);
    };
    let event = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let list = crate::builtins::child_process::proc_common::emitter_snapshot(id, &event);
    let arr = vm.alloc_array(list);
    Ok(Value::Object(arr))
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
    let out = match args.first().map(|v| v.case()) {
        Some(ValueCase::Object(r)) => {
            let mut href = String::new();
            if matches!(
                vm.heap.get(r.index()),
                Some(crate::heap::HeapObject::Ordinary { .. })
            ) {
                if let Some(s) = vm.own_value(r.index(), "href").and_then(|v| v.as_object()) {
                    if let Some(crate::heap::HeapObject::String(t)) = vm.heap.get(s.index()) {
                        href = t.clone();
                    }
                }
            }
            href
        }
        Some(v) => vm.format_value(v),
        None => String::new(),
    };
    Ok(Value::Object(vm.alloc_string(out)))
}
