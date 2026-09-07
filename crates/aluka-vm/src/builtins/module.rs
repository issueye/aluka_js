//! `module` 内置模块（Node.js 22 语义，Node.js 22 LTS 标准：
//! Node.js 22 LTS 规范）。
//!
//! - `builtinModules`：Node 22 的 68 项内置模块名（无 `node:` 前缀），
//!   数组内容与 Node.js 22 LTS 标准 输出逐字一致（`join(",")` 对拍）；
//! - `createRequire(filename)`：返回 require 函数，可加载内置模块
//!   （文件模块经 [`Vm::call_require`] 既有 CJS 链路解析）；
//! - `isBuiltin`：支持 `node:` 前缀；`constants.compileCacheStatus`、
//!   `Module` 类表面（含 `wrap` 的逐字包装文本、`_nodeModulePaths` 的
//!   node_modules 链）、`SourceMap` 最小面；
//! - 诊断/编译缓存方法面（`enableCompileCache` 等九个）：存在且返回
//!   `undefined`（对齐 Go「API 面、无真实实现」）。
//!
//! 架构限制：`Module.prototype._compile` 的真实 CJS 源码编译依赖
//! aluka-parser/aluka-compiler（ISA 契约禁止 aluka-vm 依赖），传参调用
//! 时抛说明性错误；缺参错误消息与 Go 逐字一致。
//!
//! 实例分派复用 `mod.rs::builtin_ns` 机制：Module 原型/实例标记
//! `"module:proto"`；`createRequire` 的返回函数以处理器键直调分派。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;

/// Module 原型/实例的命名空间标记（`builtin_ns` 机制）。
const NS_PROTO: &str = "module:proto";
/// 命名空间标记键。
const NS_KEY: &str = "_builtinNs";

/// 与 Node 22 / Node.js 22 LTS 标准 完全一致的内置模块名表（68 项，顺序敏感）。
const BUILTIN_MODULES: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_common",
    "_tls_wrap",
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

// `Module.prototype` 句柄（build 时暂存，构造实例时拷贝方法为自有属性）。
thread_local! {
    static MODULE_PROTO: RefCell<Option<ObjectRef>> = const { RefCell::new(None) };
}

/// `require("module")` / `require("node:module")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "module",
    build,
};

/// `Module` 类对象导出条目（同名注册进模块表，使 NativeCtor 接收者的
/// 静态方法调用经 `module_of` 命中 `Module.*` 分派键，对齐 events 模式）。
pub const MODULE_CLASS: ModuleDef = ModuleDef {
    name: "Module",
    build: build_module_class,
};

/// 从 module 单例上取 `Module` 类对象（build 顺序保证 module 先注册）。
fn build_module_class(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let module_obj = registry.module("module").ok_or_else(|| {
        let msg = vm.alloc_string("module 模块尚未初始化".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    match vm.get_property(Value::Object(module_obj), "Module")? {
        Value::Object(ctor) => Ok(ctor),
        _ => {
            let msg = vm.alloc_string("module.Module 类对象缺失".to_owned());
            Err(VmError::Thrown(Value::Object(msg)))
        }
    }
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // createRequire / isBuiltin / registerVirtualModule
    for method in ["createRequire", "isBuiltin", "registerVirtualModule"] {
        let fn_ref = vm.alloc_native_fn(&format!("module.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "module", "createRequire", create_require);
    register_handler(registry, "module", "isBuiltin", is_builtin);
    register_handler(registry, "module", "registerVirtualModule", noop);

    // builtinModules：Node 22 完整列表（数组元素为堆字符串）
    let elems: Vec<Value> = BUILTIN_MODULES
        .iter()
        .map(|n| Value::Object(vm.alloc_string((*n).to_owned())))
        .collect();
    let bm = vm.alloc_array(elems);
    set_module_prop(vm, obj, "builtinModules", Value::Object(bm))?;

    // constants.compileCacheStatus
    let ccs = vm.alloc_ordinary();
    for (k, v) in [
        ("FAILED", 0.0),
        ("ENABLED", 1.0),
        ("ALREADY_ENABLED", 2.0),
        ("DISABLED", 3.0),
    ] {
        set_module_prop(vm, ccs, k, Value::Number(v))?;
    }
    let constants = vm.alloc_ordinary();
    set_module_prop(vm, constants, "compileCacheStatus", Value::Object(ccs))?;
    set_module_prop(vm, obj, "constants", Value::Object(constants))?;

    // 诊断与编译缓存方法面（API 面，返回 undefined）
    for name in [
        "syncBuiltinESMExports",
        "registerHooks",
        "runMain",
        "enableCompileCache",
        "flushCompileCache",
        "findPackageJSON",
        "setSourceMapsSupport",
        "stripTypeScriptTypes",
        "findSourceMap",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("module.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
        register_handler(registry, "module", name, noop);
    }

    // register / getSourceMapsSupport / getCompileCacheDir
    let register_fn = vm.alloc_native_fn("module.register");
    set_module_prop(vm, obj, "register", Value::Object(register_fn))?;
    register_handler(registry, "module", "register", register_hooks);
    let sms_fn = vm.alloc_native_fn("module.getSourceMapsSupport");
    set_module_prop(vm, obj, "getSourceMapsSupport", Value::Object(sms_fn))?;
    register_handler(
        registry,
        "module",
        "getSourceMapsSupport",
        get_source_maps_support,
    );
    let ccd_fn = vm.alloc_native_fn("module.getCompileCacheDir");
    set_module_prop(vm, obj, "getCompileCacheDir", Value::Object(ccd_fn))?;
    register_handler(registry, "module", "getCompileCacheDir", noop);

    // Module 类：原型方法 + 静态方法/属性
    let proto = vm.alloc_ordinary();
    for method in ["require", "load", "_compile", "isPreloading"] {
        let fn_ref = vm.alloc_native_fn(&format!("module:proto.{method}"));
        set_module_prop(vm, proto, method, Value::Object(fn_ref))?;
    }
    let proto_ns = vm.alloc_string(NS_PROTO.to_owned());
    set_module_prop(vm, proto, NS_KEY, Value::Object(proto_ns))?;
    MODULE_PROTO.with(|slot| *slot.borrow_mut() = Some(proto));

    let module_ctor = vm.alloc_native_ctor("Module.Module", Some(proto));
    for method in [
        "runMain",
        "wrap",
        "_load",
        "_resolveFilename",
        "_nodeModulePaths",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("Module.{method}"));
        set_module_prop(vm, module_ctor, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "Module", "runMain", noop);
    register_handler(registry, "Module", "wrap", module_wrap);
    register_handler(registry, "Module", "_load", noop);
    register_handler(registry, "Module", "_resolveFilename", noop);
    register_handler(
        registry,
        "Module",
        "_nodeModulePaths",
        module_node_module_paths,
    );
    let global_paths = vm.alloc_array(Vec::new());
    set_module_prop(vm, module_ctor, "globalPaths", Value::Object(global_paths))?;
    set_module_prop(vm, obj, "Module", Value::Object(module_ctor))?;

    // SourceMap 类（最小面）：实例仅含 payload 空对象
    let source_map_ctor = vm.alloc_native_ctor("SourceMap.SourceMap", None);
    set_module_prop(vm, obj, "SourceMap", Value::Object(source_map_ctor))?;
    register_handler(registry, "SourceMap", "SourceMap", source_map_new);

    // Module 构造器（`new module.Module([id])` 经 do_construct 按名分派）
    register_handler(registry, "Module", "Module", module_new);
    // 原型方法与 createRequire 桥接
    register_handler(registry, "module:proto", "require", proto_require);
    register_handler(registry, "module:proto", "load", noop);
    register_handler(registry, "module:proto", "_compile", proto_compile);
    register_handler(
        registry,
        "module:proto",
        "isPreloading",
        proto_is_preloading,
    );
    register_handler(registry, "module", "requireBridge", require_bridge);
    Ok(obj)
}

// ---------------------------------------------------------------------------
// 顶层 API
// ---------------------------------------------------------------------------

/// `module.createRequire(filename | fileURL)`：返回 require 函数。
/// URL 对象（带 `href`）仅接受 `file:` scheme（Node ERR_INVALID_URL_SCHEME，
/// 错误消息与 Go 逐字一致）；父路径仅作兼容保留——内置模块与相对文件模块
/// 的解析复用 VM 既有 CJS 基准目录（`Vm::call_require`）。
fn create_require(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(arg) = args.first() {
        if matches!(arg, Value::Object(_)) {
            let href = vm.get_property(*arg, "href")?;
            if !matches!(href, Value::Undefined | Value::Null) {
                let s = vm.format_value(href);
                if !s.is_empty() && !s.to_ascii_lowercase().starts_with("file:") {
                    return Err(typed_error(
                        vm,
                        "aluka: type error [ERR_INVALID_URL_SCHEME]: The URL must be of scheme file",
                    ));
                }
            }
        }
    }
    let bridge = vm.alloc_native_fn("module.requireBridge");
    Ok(Value::Object(bridge))
}

/// `createRequire` 返回的 require 函数体：与顶层 `require` 同链路
/// （内置模块优先，其余按 CJS 解析 .bc）。
fn require_bridge(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let spec = args.first().copied().unwrap_or(Value::Undefined);
    vm.call_require(spec)
}

/// `module.isBuiltin(specifier)`：支持 `node:` 前缀；缺参/非内置 → false。
fn is_builtin(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let hit = match args.first() {
        None => false,
        Some(v) => {
            let spec = vm.format_value(*v);
            let stripped = spec.strip_prefix("node:").unwrap_or(&spec);
            BUILTIN_MODULES.contains(&stripped)
        }
    };
    Ok(Value::Boolean(hit))
}

/// `module.register([specifier][, parentURL][, options])`：缺 specifier 抛
/// TypeError（消息与 Go 逐字一致）；注册 loader hooks 链路本阶段为 API 面。
fn register_hooks(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(typed_error(vm, "register requires a specifier"));
    }
    Ok(Value::Undefined)
}

/// `module.getSourceMapsSupport()`：Go 同返回 false（无 sourcemap 支持）。
fn get_source_maps_support(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// 通用 API 面：存在且返回 `undefined`（对齐 Go 的诊断/缓存方法族）。
fn noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// Module 类（构造器 / 原型 / 静态）
// ---------------------------------------------------------------------------

/// `new module.Module([id])`：Node 语义——`id = filename 实参`、
/// `filename` 初始 `null`、`loaded = false`、`children`/`paths` 空数组、
/// `exports` 空对象；原型方法拷贝为自有属性（对齐 Go）。
fn module_new(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let inst = vm.alloc_ordinary();
    let exports = vm.alloc_ordinary();
    set_module_prop(vm, inst, "exports", Value::Object(exports))?;
    let id = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let id_ref = vm.alloc_string(id);
    set_module_prop(vm, inst, "id", Value::Object(id_ref))?;
    set_module_prop(vm, inst, "filename", Value::Null)?;
    set_module_prop(vm, inst, "loaded", Value::Boolean(false))?;
    let children = vm.alloc_array(Vec::new());
    set_module_prop(vm, inst, "children", Value::Object(children))?;
    let paths = vm.alloc_array(Vec::new());
    set_module_prop(vm, inst, "paths", Value::Object(paths))?;
    copy_proto_methods(vm, inst)?;
    let inst_ns = vm.alloc_string(NS_PROTO.to_owned());
    set_module_prop(vm, inst, NS_KEY, Value::Object(inst_ns))?;
    Ok(Value::Object(inst))
}

/// `Module.prototype.require(spec)`：以 `/` 父路径语义走 require 链路
/// （Go `MakeRequireFunc("/")`；Rust 复用 VM 既有 CJS 链路）。
fn proto_require(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    require_bridge(vm, args)
}

/// `Module.prototype._compile(code, filename)`：缺源码抛 TypeError（与 Go
/// 逐字一致）；真实 CJS 编译依赖 parser/compiler（ISA 契约禁止），本阶段
/// 抛说明性错误。
fn proto_compile(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(typed_error(vm, "_compile requires source code"));
    }
    Err(error_instance(
        vm,
        "module._compile: source compilation is not supported by aluka-vm \
         (aluka-parser/aluka-compiler dependency is forbidden by the ISA contract)",
    ))
}

/// `Module.prototype.isPreloading()`：Go 同返回 false。
fn proto_is_preloading(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// `Module.wrap(code)`：CJS 包装文本逐字对齐 Go：
/// `(function (exports, require, module, __filename, __dirname) { {code}\n});`
/// 缺参返回空串。
fn module_wrap(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let wrapped = match args.first() {
        None | Some(Value::Undefined) => String::new(),
        Some(v) => format!(
            "(function (exports, require, module, __filename, __dirname) {{ {}\n}});",
            vm.format_value(*v)
        ),
    };
    Ok(Value::Object(vm.alloc_string(wrapped)))
}

/// `Module._nodeModulePaths(start)`：从 start（默认 `.`，先转绝对路径）逐级
/// 向上收集 `<dir>/node_modules`，到根停止（对齐 Go `filepath.Abs/Dir` 链）。
fn module_node_module_paths(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let raw = match args.first() {
        Some(v) => {
            let s = vm.format_value(*v);
            if s.is_empty() { ".".to_owned() } else { s }
        }
        None => ".".to_owned(),
    };
    let mut dir = std::path::PathBuf::from(&raw);
    if !dir.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            dir = cwd.join(dir);
        }
    }
    let mut elems: Vec<Value> = Vec::new();
    loop {
        let path = dir.join("node_modules");
        elems.push(Value::Object(vm.alloc_string(path.display().to_string())));
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => break,
        }
    }
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `new module.SourceMap()`：最小面实例（`payload` 空对象，对齐 Go）。
fn source_map_new(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let inst = vm.alloc_ordinary();
    let payload = vm.alloc_ordinary();
    set_module_prop(vm, inst, "payload", Value::Object(payload))?;
    Ok(Value::Object(inst))
}

// ---------------------------------------------------------------------------
// 内部辅助
// ---------------------------------------------------------------------------

/// 把原型上的方法 NativeFn 拷贝为实例自有属性（排除 `_builtinNs` 标记，
/// 对齐 Go「实例继承 prototype 方法」的拷贝式实现）。
fn copy_proto_methods(vm: &mut Vm, inst: ObjectRef) -> Result<(), VmError> {
    if let Some(proto) = MODULE_PROTO.with(|slot| slot.borrow().as_ref().copied()) {
        if matches!(
            vm.heap.get(proto.index()),
            Some(crate::heap::HeapObject::Ordinary { .. })
        ) {
            let methods: Vec<(String, Value)> = vm
                .own_entries(proto.index())
                .into_iter()
                .filter(|(k, _)| k.as_str() != NS_KEY)
                .collect();
            for (k, v) in methods {
                set_module_prop(vm, inst, &k, v)?;
            }
        }
    }
    Ok(())
}

/// 抛 `name: "TypeError"` 的错误实例（Go `engine.ErrTypeError` 家族）。
fn typed_error(vm: &mut Vm, message: &str) -> VmError {
    let obj = vm.alloc_error_instance(message);
    let name = vm.alloc_string("TypeError".to_owned());
    let _ = vm.set_property(Value::Object(obj), "name", Value::Object(name));
    VmError::Thrown(Value::Object(obj))
}

/// 抛 `name: "Error"` 的错误实例。
fn error_instance(vm: &mut Vm, message: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(message)))
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = create_require;
        let _: crate::builtins::BuiltinHandler = is_builtin;
        let _: crate::builtins::BuiltinHandler = register_hooks;
        let _: crate::builtins::BuiltinHandler = get_source_maps_support;
        let _: crate::builtins::BuiltinHandler = noop;
        let _: crate::builtins::BuiltinHandler = module_new;
        let _: crate::builtins::BuiltinHandler = proto_require;
        let _: crate::builtins::BuiltinHandler = proto_compile;
        let _: crate::builtins::BuiltinHandler = proto_is_preloading;
        let _: crate::builtins::BuiltinHandler = module_wrap;
        let _: crate::builtins::BuiltinHandler = module_node_module_paths;
        let _: crate::builtins::BuiltinHandler = source_map_new;
        let _: crate::builtins::BuiltinHandler = require_bridge;
    }

    /// builtinModules 表必须恰好 68 项（Node.js 22 LTS 标准 实测数量）。
    #[test]
    fn builtin_modules_count() {
        assert_eq!(BUILTIN_MODULES.len(), 68);
    }
}
