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

/// `process` 的元信息面：`version`/`versions`/`platform`/`arch`/`pid`/
/// `execPath`/`title`/`uptime`/`hrtime`/`exitCode`。
///
/// 这些字段是真实生态的**能力探测入口**（包管理器、平台分支、特性开关、
/// 计时器都先读它们）——此前 `process` 只有事件面与 argv/cwd/exit，
/// `process.platform` 读出来是 undefined，任何 `if (process.platform === ...)`
/// 分支都会静默走错路。
fn install_process_metadata(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<(), VmError> {
    let Some(process_obj) = vm.process_object else {
        return Ok(());
    };
    let target = Value::Object(process_obj);
    // `version`：与权威 oracle Node.js 22 LTS 对齐（v22.23.1）
    let version = Value::Object(vm.alloc_string("v22.23.1".to_owned()));
    let _ = vm.set_property(target, "version", version);
    let versions = vm.alloc_ordinary();
    for (k, v) in [
        ("node", "22.23.1"),
        ("aluka", env!("CARGO_PKG_VERSION")),
        ("v8", "12.4.254.21-node.30"),
    ] {
        let s = vm.alloc_string(v.to_owned());
        let _ = vm.set_property(Value::Object(versions), k, Value::Object(s));
    }
    let _ = vm.set_property(target, "versions", Value::Object(versions));
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let s = vm.alloc_string(platform.to_owned());
    let _ = vm.set_property(target, "platform", Value::Object(s));
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    let s = vm.alloc_string(arch.to_owned());
    let _ = vm.set_property(target, "arch", Value::Object(s));
    let _ = vm.set_property(target, "pid", Value::Number(f64::from(std::process::id())));
    let s = vm.alloc_string(
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    let _ = vm.set_property(target, "execPath", Value::Object(s));
    let s = vm.alloc_string(String::new());
    let _ = vm.set_property(target, "title", Value::Object(s));
    let _ = vm.set_property(target, "exitCode", Value::Undefined);
    let hrtime_fn = vm.alloc_native_fn("process.hrtime");
    let bigint_fn = vm.alloc_native_fn("process.hrtime.bigint");
    vm.set_native_fn_property(hrtime_fn, "bigint", Value::Object(bigint_fn));
    let _ = vm.set_property(target, "hrtime", Value::Object(hrtime_fn));
    register_handler(registry, "process", "hrtime", process_hrtime);
    register_handler(registry, "process", "hrtime.bigint", process_hrtime_bigint);
    Ok(())
}

/// 进程启动锚点（`hrtime`/`uptime` 的统一时基）。
static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn process_start() -> &'static std::time::Instant {
    PROCESS_START.get_or_init(std::time::Instant::now)
}

/// `process.hrtime([prev])` → `[秒, 纳秒]`（相对进程启动；传 prev 时返回差值）。
fn process_hrtime(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let now = process_start().elapsed();
    let (secs, nanos) = if let Some(prev) = args.first().copied() {
        let vals = match prev.as_object() {
            Some(r) => vm.array_elements(r.0 as usize),
            None => Vec::new(),
        };
        let psecs = vals.first().copied().map_or(0.0, |v| vm.to_number_value(v));
        let pnanos = vals.get(1).copied().map_or(0.0, |v| vm.to_number_value(v));
        let total = (now.as_secs_f64() - psecs) - pnanos / 1e9;
        (total.trunc(), (total.fract() * 1e9).max(0.0))
    } else {
        (now.as_secs() as f64, f64::from(now.subsec_nanos()))
    };
    let elems = vec![Value::Number(secs), Value::Number(nanos.trunc())];
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `process.hrtime.bigint()` → 进程启动以来的纳秒 BigInt。
fn process_hrtime_bigint(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let nanos = process_start().elapsed().as_nanos();
    let big = vm.alloc_bigint(nanos.to_string());
    Ok(Value::Object(big))
}

fn build_process(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    install_process_metadata(vm, registry)?;
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
        Some(v) if !v.is_undefined() => Some(vm.to_property_key(*v)),
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
    for name in [
        "parse",
        "resolve",
        "resolveObject",
        "format",
        "URL",
        "Url",
        "URLSearchParams",
        "domainToASCII",
        "domainToUnicode",
        "fileURLToPath",
        "fileURLToPathBuffer",
        "pathToFileURL",
        "urlToHttpOptions",
    ] {
        let f = vm.alloc_native_fn(&format!("url.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
    }
    register_handler(registry, "url", "parse", url_parse);
    register_handler(registry, "url", "resolve", url_resolve);
    register_handler(registry, "url", "resolveObject", url_resolve);
    register_handler(registry, "url", "format", url_format);
    register_handler(registry, "url", "fileURLToPath", url_file_url_to_path);
    register_handler(registry, "url", "fileURLToPathBuffer", url_file_url_to_path);
    register_handler(registry, "url", "pathToFileURL", url_path_to_file_url);
    register_handler(registry, "url", "domainToASCII", url_domain_to_ascii);
    register_handler(registry, "url", "domainToUnicode", url_domain_to_unicode);
    register_handler(registry, "url", "urlToHttpOptions", url_to_http_options);
    // URL / URLSearchParams：全局构造器复用（同一实例面）
    if let Some(url_ctor) = vm.globals.get("URL").copied() {
        set_module_prop(vm, obj, "URL", url_ctor)?;
    }
    // legacy `Url` 构造器（Node `require('url').Url`）：`new Url()` 产出**空**
    // URL 记录，字段逐个赋值、`instanceof Url` 成立——parseurl 等包据此判型
    // 并填充字段（此前误接为 WHATWG 构造器，`new Url()` 抛 "Invalid URL"，
    // 使 express 的解析层直接抛错、路由全落 404）
    let legacy_proto = {
        let base = vm.object_prototype.unwrap_or_else(|| vm.alloc_ordinary());
        vm.alloc_ordinary_with_exact_proto(Some(base))
    };
    let legacy_ctor = vm.alloc_native_ctor("Url", Some(legacy_proto));
    set_module_prop(vm, obj, "Url", Value::Object(legacy_ctor))?;
    registry.dispatch.insert("Url".to_owned(), url_legacy_ctor);
    if let Some(usp_ctor) = vm.globals.get("URLSearchParams").copied() {
        set_module_prop(vm, obj, "URLSearchParams", usp_ctor)?;
    }
    Ok(obj)
}

/// legacy `Url` 构造：`new Url()` → 空 URL 记录（字段与 Node 的
/// `Url.prototype` 初始面一致：协议/主机/端口等为 null，path/href 置空串）。
fn url_legacy_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let inst = vm.alloc_ordinary();
    // 原型链接：`parsed instanceof Url` 判型（parseurl 的 fresh() 依赖）
    let ctor = vm
        .builtin_registry
        .module("url")
        .and_then(|m| vm.get_property(Value::Object(m), "Url").ok())
        .unwrap_or(Value::Undefined);
    if let ValueCase::Object(c) = ctor.case() {
        if let ValueCase::Object(proto) = vm.get_property(Value::Object(c), "prototype")?.case() {
            vm.set_prototype_of(Value::Object(inst), Some(proto));
        }
    }
    let target = Value::Object(inst);
    for key in [
        "protocol", "slashes", "auth", "host", "port", "hostname", "hash", "search", "query",
    ] {
        vm.set_property(target, key, Value::Null)?;
    }
    for key in ["pathname", "path", "href"] {
        let empty = vm.alloc_string(String::new());
        vm.set_property(target, key, Value::Object(empty))?;
    }
    Ok(target)
}

/// `url.domainToASCII(domain)`：IDNA ToASCII（小写化 + 逐标签 xn-- 编码）。
fn url_domain_to_ascii(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let raw = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let lowered = raw.to_lowercase();
    let out = crate::builtins::punycode::map_domain(&lowered, |label| {
        if crate::builtins::punycode::has_non_ascii(label) {
            if let Ok(enc) = crate::builtins::punycode::punycode_encode(label) {
                return format!("xn--{enc}");
            }
        }
        label.to_owned()
    });
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `url.domainToUnicode(domain)`：xn-- 标签解码回 Unicode。
fn url_domain_to_unicode(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let raw = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let out = crate::builtins::punycode::map_domain(&raw, |label| {
        if let Some(rest) = label.strip_prefix("xn--") {
            if let Ok(dec) = crate::builtins::punycode::punycode_decode(&rest.to_lowercase()) {
                return dec;
            }
        }
        label.to_owned()
    });
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `url.urlToHttpOptions(url)`：URL 对象 → http.request 选项对象。
fn url_to_http_options(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let source = args.first().copied().unwrap_or(Value::Undefined);
    let out = vm.alloc_ordinary();
    if source.as_object().is_none() {
        return Ok(Value::Object(out));
    }
    for key in ["protocol", "hostname", "hash", "search", "pathname", "href"] {
        let v = vm.get_property(source, key)?;
        vm.set_property(Value::Object(out), key, v)?;
    }
    let port = vm.get_property(source, "port")?;
    let port_text = vm.format_value(port);
    let port_num = if port == Value::Undefined || port_text.is_empty() {
        Value::Undefined
    } else {
        Value::Number(vm.to_number_value(port))
    };
    vm.set_property(Value::Object(out), "port", port_num)?;
    let pathname = vm.get_property(source, "pathname")?;
    let search = vm.get_property(source, "search")?;
    let path_text = format!("{}{}", vm.format_value(pathname), vm.format_value(search));
    let path_val = vm.alloc_string(path_text);
    vm.set_property(Value::Object(out), "path", Value::Object(path_val))?;
    if let Ok(auth) = vm.get_property(source, "username") {
        let user = vm.format_value(auth);
        if !user.is_empty() {
            let pass_value = vm.get_property(source, "password")?;
            let pass = vm.format_value(pass_value);
            let auth_text = if pass.is_empty() {
                user
            } else {
                format!("{user}:{pass}")
            };
            let auth_val = vm.alloc_string(auth_text);
            vm.set_property(Value::Object(out), "auth", Value::Object(auth_val))?;
        }
    }
    Ok(Value::Object(out))
}

/// 取 `file:` URL 的路径文本：接受 URL 实例（读 `href`）或字符串。
///
/// 注意堆字符串在 Value 层同样是「对象」——必须按堆对象种类区分，否则
/// `fileURLToPath(import.meta.url)` 会被当作 URL 实例去读 `href`
/// （读到 undefined，报 "The URL must be of scheme file: undefined"）。
fn file_url_text(vm: &mut Vm, value: Value) -> Result<String, VmError> {
    if let Some(r) = value.as_object() {
        let is_string = matches!(
            vm.heap.get(r.0 as usize),
            Some(crate::heap::HeapObject::String(_))
        );
        if !is_string {
            let href = vm.get_property(value, "href")?;
            return Ok(vm.format_value(href));
        }
    }
    Ok(vm.format_value(value))
}

/// `file:` URL → 本地路径（Node `fileURLToPath` 语义）。
///
/// - 仅接受 `file:` 协议（其余抛 `ERR_INVALID_URL_SCHEME` 的 TypeError）；
/// - 百分号解码；Windows 盘符形态 `/C:/x` 还原为 `C:\x`；
/// - 含编码斜杠（`%2F`/`%5C`）或未定义主机的 URL 抛错（Node 同款诊断）。
fn file_url_to_path_text(raw: &str) -> Result<String, String> {
    let rest: String = match raw.strip_prefix("file://") {
        Some(r) => r.to_owned(),
        None => {
            if raw.starts_with("file:") {
                raw.trim_start_matches("file:").to_owned()
            } else {
                return Err(format!("The URL must be of scheme file: {raw}"));
            }
        }
    };
    let (host, path) = match rest.find('/') {
        Some(0) => (String::new(), rest.clone()),
        Some(i) => (rest[..i].to_owned(), rest[i..].to_owned()),
        None => (rest.clone(), String::new()),
    };
    if !host.is_empty() && host != "localhost" {
        return Err(format!(
            "File URL host must be \"localhost\" or empty: {raw}"
        ));
    }
    let decoded = percent_decode(&path);
    if decoded.contains('/') && path.contains("%2F") {
        return Err(format!(
            "File URL path must not include encoded / characters: {raw}"
        ));
    }
    if cfg!(windows) {
        // `/C:/dir/file` → `C:\dir\file`（盘符在首段且为单字母）
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && decoded.starts_with('/') && bytes[2] == b':' {
            let drive = decoded[1..2].to_owned();
            let tail = decoded[3..].replace('/', "\\");
            return Ok(format!("{drive}:{tail}"));
        }
    }
    Ok(decoded)
}

/// 轻量百分号解码（`fileURLToPath` 只需处理路径中的转义字节）。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `fileURLToPath(url)` → 路径字符串。
fn url_file_url_to_path(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let value = args.first().copied().unwrap_or(Value::Undefined);
    let raw = file_url_text(vm, value)?;
    match file_url_to_path_text(&raw) {
        Ok(path) => {
            let s = vm.alloc_string(path);
            Ok(Value::Object(s))
        }
        Err(message) => {
            let msg = vm.alloc_string(message);
            Err(VmError::Thrown(Value::Object(msg)))
        }
    }
}

/// `pathToFileURL(path)` → URL 对象（`href` 为 `file:///...`，已转义）。
fn url_path_to_file_url(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let raw = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let href = path_text_to_file_url(&raw);
    // 复用 URL 构造器实体（`new URL(href)` 同源），保证返回的是真正的
    // URL 实例（`instanceof URL` / `href` 访问器 / `searchParams` 齐备）
    let url_arg = Value::Object(vm.alloc_string(href));
    crate::builtins::global::url_obj::url_ctor(vm, &[url_arg])
}

/// 路径文本 → `file:` URL（Node `pathToFileURL` 语义：绝对化、反斜杠转正斜杠、
/// 逐段百分号转义；目录路径补尾斜杠）。
fn path_text_to_file_url(raw: &str) -> String {
    let path = std::path::Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(path)
    };
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let mut encoded = String::from("file://");
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@' => encoded.push(byte as char),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    if absolute.is_dir() && !encoded.ends_with('/') {
        encoded.push('/');
    }
    encoded
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
    let out = match args.first().copied() {
        // 传对象（含 WHATWG `URL` 实例）时取 `href`——**必须走属性读取**
        // 而非 `own_value` 直查：`URL` 的 href 是访问器（挂在 getter 表
        // 里），直查自有数据槽只会拿到空串（`url.format(new URL(...))`
        // 曾恒返回 ""）。
        Some(v) if v.as_object().is_some() => {
            let href = vm.get_property(v, "href").unwrap_or(Value::Undefined);
            if matches!(href, Value::Undefined | Value::Null) {
                String::new()
            } else {
                vm.format_value(href)
            }
        }
        Some(v) => vm.format_value(v),
        None => String::new(),
    };
    Ok(Value::Object(vm.alloc_string(out)))
}
