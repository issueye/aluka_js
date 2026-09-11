//! `wasi` 内置模块（Phase 8）：实验性（stability 1）的 WASI preview1 方法面。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodeutil/wasi.go`）。aluka 无 WebAssembly 运行时，
//! 因此 WASI 只提供与 Node 22 一致的类/方法面：
//! - `WASI(options)`：校验 options（version/args/env/preopens/stdin/stdout/
//!   stderr），错误码/消息与 Node 对齐（ERR_INVALID_ARG_TYPE /
//!   ERR_INVALID_ARG_VALUE / ERR_OUT_OF_RANGE）；
//! - `instance.wasiImport`：46 个 preview1 系统调用函数（调用前抛
//!   ERR_WASI_NOT_STARTED，Node 语义）；
//! - `start(instance)` / `initialize(instance)`：校验顺序与 Node 一致
//!   （started 标记 → instance/exports/memory 校验 → _start/_initialize）；
//! - `getImportObject()`：`{ wasi_snapshot_preview1 | wasi_unstable: wasiImport }`。
//!
//! 已知差异（对齐 Go 实现自身的 knownDifference）：无 WASM 运行时，
//! `start`/`initialize` 在内存校验后无法真正执行 WASM，抛
//! ERR_WASI_NOT_IMPLEMENTED；实验警告不输出（避免污染差分输出）。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("wasi")` / `require("node:wasi")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "wasi",
    build,
};

/// WASI preview1 导入对象的 46 个系统调用函数名。
const PREVIEW1_FUNCTIONS: [&str; 46] = [
    "args_get",
    "args_sizes_get",
    "clock_res_get",
    "clock_time_get",
    "environ_get",
    "environ_sizes_get",
    "fd_advise",
    "fd_allocate",
    "fd_close",
    "fd_datasync",
    "fd_fdstat_get",
    "fd_fdstat_set_flags",
    "fd_fdstat_set_rights",
    "fd_filestat_get",
    "fd_filestat_set_size",
    "fd_filestat_set_times",
    "fd_pread",
    "fd_prestat_dir_name",
    "fd_prestat_get",
    "fd_pwrite",
    "fd_read",
    "fd_readdir",
    "fd_renumber",
    "fd_seek",
    "fd_sync",
    "fd_tell",
    "fd_write",
    "path_create_directory",
    "path_filestat_get",
    "path_filestat_set_times",
    "path_link",
    "path_open",
    "path_readlink",
    "path_remove_directory",
    "path_rename",
    "path_symlink",
    "path_unlink_file",
    "poll_oneoff",
    "proc_exit",
    "proc_raise",
    "random_get",
    "sched_yield",
    "sock_accept",
    "sock_recv",
    "sock_send",
    "sock_shutdown",
];

/// 构建 node:wasi 模块导出对象（`{ WASI }`）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // WASI 类：NativeCtor 名称即分派键——`new WASI(opts)` 经 do_construct
    // 的 `lookup(ctor_name)` 命中处理器；prototype 挂 constructor 支持 instanceof。
    let proto = vm.alloc_ordinary();
    let ctor = vm.alloc_native_ctor("wasi.WASI.ctor", None);
    let ctor_val = Value::Object(ctor);
    let proto_val = Value::Object(proto);
    let _ = vm.set_property(Value::Object(proto), "constructor", ctor_val);
    let _ = vm.set_property(Value::Object(ctor), "prototype", proto_val);
    let _ = vm.set_property(Value::Object(obj), "WASI", ctor_val);

    register_handler(registry, "wasi.WASI", "ctor", wasi_ctor);
    register_handler(registry, "wasi:inst", "start", wasi_start);
    register_handler(registry, "wasi:inst", "initialize", wasi_initialize);
    register_handler(
        registry,
        "wasi:inst",
        "getImportObject",
        wasi_get_import_object,
    );
    for name in PREVIEW1_FUNCTIONS {
        register_handler(registry, "wasi:import", name, wasi_import_stub);
    }

    Ok(obj)
}

/// 构造携带 Node 风格错误码（.code）的异常对象。
fn code_error(vm: &mut Vm, code: &str, msg: &str) -> VmError {
    let err = vm.alloc_ordinary();
    let msg_val = Value::Object(vm.alloc_string(msg.to_owned()));
    let _ = vm.set_property(Value::Object(err), "message", msg_val);
    let code_val = Value::Object(vm.alloc_string(code.to_owned()));
    let _ = vm.set_property(Value::Object(err), "code", code_val);
    VmError::Thrown(Value::Object(err))
}

/// 是否为堆字符串值。
fn is_string(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::String(_))))
}

/// 是否为普通对象（Ordinary 堆对象）。
fn is_ordinary(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })))
}

/// 渲染 Node 的 "Received ..." 片段（对齐 Go `wasiTypeString`）。
fn type_string(vm: &Vm, v: Value) -> String {
    match v.case() {
        ValueCase::Undefined => "undefined".to_owned(),
        ValueCase::Null => "null".to_owned(),
        ValueCase::Number(n) => format!("type number ({})", vm.format_value(Value::Number(n))),
        ValueCase::Boolean(b) => format!("type boolean ({b})"),
        ValueCase::Object(r) => match vm.heap.get(r.index()) {
            Some(HeapObject::String(s)) => format!("type string ('{s}')"),
            Some(HeapObject::Closure { .. })
            | Some(HeapObject::NativeCtor { .. })
            | Some(HeapObject::NativeFn { .. }) => "type function".to_owned(),
            _ => "type object".to_owned(),
        },
    }
}

/// `new WASI(options)`：构造函数主体（校验 + 实例装配）。
fn wasi_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let options = args.first().copied().unwrap_or(Value::Undefined);
    let opts_val = match options {
        Value::Undefined | Value::Null => None,
        v if is_ordinary(vm, v) => Some(v),
        _ => {
            return Err(code_error(
                vm,
                "ERR_INVALID_ARG_TYPE",
                &format!(
                    "The \"options\" argument must be of type object. Received {}",
                    type_string(vm, options)
                ),
            ));
        }
    };

    // options.version：必填字符串。
    let version = match opts_val {
        Some(o) => {
            let ver = vm.get_property(o, "version")?;
            if is_string(vm, ver) {
                let ValueCase::Object(s) = ver.case() else {
                    unreachable!()
                };
                match vm.heap.get(s.index()) {
                    Some(HeapObject::String(text)) => text.clone(),
                    _ => String::new(),
                }
            } else {
                return Err(code_error(
                    vm,
                    "ERR_INVALID_ARG_TYPE",
                    &format!(
                        "The \"options.version\" property must be of type string. Received {}",
                        type_string(vm, ver)
                    ),
                ));
            }
        }
        None => {
            return Err(code_error(
                vm,
                "ERR_INVALID_ARG_TYPE",
                "The \"options.version\" property must be of type string. Received undefined",
            ));
        }
    };
    let binding_name = match version.as_str() {
        "unstable" => "wasi_unstable",
        "preview1" => "wasi_snapshot_preview1",
        other => {
            return Err(code_error(
                vm,
                "ERR_INVALID_ARG_VALUE",
                &format!(
                    "The property 'options.version' unsupported WASI version. Received '{other}'"
                ),
            ));
        }
    };

    // options.args：数组。
    if let Some(o) = opts_val {
        let av = vm.get_property(o, "args")?;
        if !av.is_undefined() {
            let is_array = matches!(av.case(), ValueCase::Object(ar)
                if matches!(vm.heap.get(ar.index()), Some(HeapObject::Array { .. })));
            if !is_array {
                return Err(code_error(
                    vm,
                    "ERR_INVALID_ARG_TYPE",
                    &format!(
                        "The \"options.args\" property must be an instance of Array. Received {}",
                        type_string(vm, av)
                    ),
                ));
            }
        }
    }

    // options.env / options.preopens：对象。
    for key in ["env", "preopens"] {
        if let Some(o) = opts_val {
            let ev = vm.get_property(o, key)?;
            if !matches!(ev, Value::Undefined) && !is_ordinary(vm, ev) {
                return Err(code_error(
                    vm,
                    "ERR_INVALID_ARG_TYPE",
                    &format!(
                        "The \"options.{key}\" property must be of type object. Received {}",
                        type_string(vm, ev)
                    ),
                ));
            }
        }
    }

    // options.stdin/stdout/stderr：0..2^31-1 整数。
    for key in ["stdin", "stdout", "stderr"] {
        let v = match opts_val {
            Some(o) => vm.get_property(o, key)?,
            None => Value::Undefined,
        };
        if matches!(v, Value::Undefined) {
            continue;
        }
        let ok_int = matches!(v.case(), ValueCase::Number(n)
            if n.fract() == 0.0 && (0.0..=2147483647.0).contains(&n));
        if !ok_int {
            let received = match v.case() {
                ValueCase::Number(n) => n as i64,
                _ => -1,
            };
            return Err(code_error(
                vm,
                "ERR_OUT_OF_RANGE",
                &format!(
                    "The value of \"options.{key}\" is out of range. It must be >= 0 && <= 2147483647. Received {received}"
                ),
            ));
        }
    }

    // 实例对象：原型链接到 WASI.prototype（支持 instanceof）；
    // `_builtinNs` 实例分派（引擎 CALL_METHOD 无属性回退）。
    let proto_ref = wasi_proto(vm);
    let inst = vm.alloc_ordinary_with_proto(proto_ref);
    let ns_val = Value::Object(vm.alloc_string("wasi:inst".to_owned()));
    set_module_prop(vm, inst, "_builtinNs", ns_val)?;
    set_module_prop(vm, inst, "_started", Value::Boolean(false))?;
    let binding_val = Value::Object(vm.alloc_string(binding_name.to_owned()));
    set_module_prop(vm, inst, "_bindingName", binding_val)?;

    // wasiImport：46 个系统调用函数（每实例新建，对齐 Go）。
    let wasi_import = vm.alloc_ordinary();
    let import_ns = Value::Object(vm.alloc_string("wasi:import".to_owned()));
    let _ = vm.set_property(Value::Object(wasi_import), "_builtinNs", import_ns);
    for name in PREVIEW1_FUNCTIONS {
        let fn_ref = vm.alloc_native_fn(&format!("wasi:import.{name}"));
        set_module_prop(vm, wasi_import, name, Value::Object(fn_ref))?;
    }
    set_module_prop(vm, inst, "wasiImport", Value::Object(wasi_import))?;

    for (prop, name) in [
        ("start", "wasi:inst.start"),
        ("initialize", "wasi:inst.initialize"),
        ("getImportObject", "wasi:inst.getImportObject"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, inst, prop, Value::Object(fn_ref))?;
    }

    Ok(Value::Object(inst))
}

/// 读取模块单例上 WASI 类的 prototype 对象。
fn wasi_proto(vm: &mut Vm) -> Option<ObjectRef> {
    let module = vm.builtin_registry.module("wasi")?;
    let ctor = vm.get_property(Value::Object(module), "WASI").ok()?;
    let proto = vm.get_property(ctor, "prototype").ok()?;
    match proto.case() {
        ValueCase::Object(p) => Some(p),
        _ => None,
    }
}

/// `start(instance)` / `initialize(instance)` 共用的 setupInstance 校验：
/// instance/exports 为对象、exports.memory 为 WebAssembly.Memory（aluka 无
/// Memory 类，任何内存值都无法通过——以错误路径对齐 Node）。
fn setup_instance(vm: &mut Vm, args: &[Value]) -> Result<(), VmError> {
    let instance = args.first().copied().unwrap_or(Value::Undefined);
    if !is_ordinary(vm, instance) {
        return Err(code_error(
            vm,
            "ERR_INVALID_ARG_TYPE",
            &format!(
                "The \"instance\" argument must be of type object. Received {}",
                type_string(vm, instance)
            ),
        ));
    }
    let exports = vm.get_property(instance, "exports")?;
    if !is_ordinary(vm, exports) {
        return Err(code_error(
            vm,
            "ERR_INVALID_ARG_TYPE",
            &format!(
                "The \"instance.exports\" property must be of type object. Received {}",
                type_string(vm, exports)
            ),
        ));
    }
    let _ = vm.get_property(exports, "memory")?;
    Err(code_error(
        vm,
        "ERR_INVALID_ARG_TYPE",
        "\"instance.exports.memory\" property must be a WebAssembly.Memory object",
    ))
}

/// started 标记检查 + 置位（对齐 Go：先置位后校验，失败路径保持已启动）。
fn check_and_mark_started(vm: &mut Vm) -> Result<(), VmError> {
    let receiver = current_receiver();
    if vm.get_property(receiver, "_started")?.as_bool() == Some(true) {
        return Err(code_error(
            vm,
            "ERR_WASI_ALREADY_STARTED",
            "WASI instance has already started",
        ));
    }
    let _ = vm.set_property(receiver, "_started", Value::Boolean(true));
    Ok(())
}

/// `wasi.start(instance)`。
fn wasi_start(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    check_and_mark_started(vm)?;
    setup_instance(vm, args)?;
    // 无 WASM 运行时：即使内存校验通过也无法执行 _start。
    Err(code_error(
        vm,
        "ERR_WASI_NOT_IMPLEMENTED",
        "aluka: WASI _start requires a WebAssembly runtime (see docs/adr/WASI-WASM.md)",
    ))
}

/// `wasi.initialize(instance)`。
fn wasi_initialize(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    check_and_mark_started(vm)?;
    setup_instance(vm, args)?;
    Err(code_error(
        vm,
        "ERR_WASI_NOT_IMPLEMENTED",
        "aluka: WASI _initialize requires a WebAssembly runtime (see docs/adr/WASI-WASM.md)",
    ))
}

/// `wasi.getImportObject()`：`{ [bindingName]: wasiImport }`。
fn wasi_get_import_object(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let binding = vm.get_property(receiver, "_bindingName")?;
    let wasi_import = vm.get_property(receiver, "wasiImport")?;
    let io = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(io), &vm.format_value(binding), wasi_import);
    Ok(Value::Object(io))
}

/// wasiImport 46 个系统调用 stub：始终抛 ERR_WASI_NOT_STARTED。
fn wasi_import_stub(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Err(code_error(
        vm,
        "ERR_WASI_NOT_STARTED",
        "wasi.start() has not been called",
    ))
}
