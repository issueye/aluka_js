//! `vm` 内置模块（Node.js 22 语义，Node.js 22 LTS 标准：Node.js 22 LTS 规范）。
//!
//! # 架构限制（本阶段）
//! aluka-vm 执行的是字节码；在运行时编译 JS 源码需要 aluka-parser /
//! aluka-compiler，而 ISA 契约分层纪律**禁止 aluka-vm 依赖这两个 crate**。
//! 因此 `runInThisContext` / `runInContext` / `runInNewContext` /
//! `compileFunction` 的调用结果、以及 `Script.prototype.run*` 的真实源码求值
//! **本阶段不实现**（调用时抛说明性错误，见 [`source_exec_error`]）。
//! 本模块提供 Node.js 22 LTS 标准 可确定性对拍的表面：
//! - 全部导出函数的存在性/typeof（runInThisContext、createContext、isContext、
//!   runInContext、runInNewContext、compileFunction、measureMemory、Script、
//!   constants、createScript）；
//! - `createContext` 返回 contextified 对象（有 sandbox 返回同一对象，幂等；
//!   无 sandbox 新建），`isContext` 按自有 `_builtinNs` 标记判定；
//! - `Script` 构造器表面：prototype 方法、`cachedDataRejected`、
//!   `createCachedData()`（返回源码字节的 Buffer，Go 同为源码字节）、
//!   非 Script 实例调用的错误消息；
//! - 缺 context / context 无效时的错误消息（求值前抛出，与 Go 逐字一致）。
//!
//! 实例分派复用 `mod.rs::builtin_ns` 机制：context 对象标记 `"vm:context"`，
//! Script 原型/实例标记 `"vm:script"`；Script 源数据存 thread_local 表
//! （对齐 timers.rs 静态状态模式，Go 侧为 `vmState.scripts`）。

use crate::builtins::buffer;
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// 实例分派命名空间标记键（见 `builtins/mod.rs` 的 `builtin_ns`）。
const NS_KEY: &str = "_builtinNs";
/// vm context 标记（`isContext` 判定依据）。
const NS_CONTEXT: &str = "vm:context";
/// vm.Script 原型/实例标记（方法分派 + 实例判定）。
const NS_SCRIPT: &str = "vm:script";
/// Go 侧 Script 默认文件名（`vm.go` 中字面量）。
const DEFAULT_FILENAME: &str = "evalmachine.<anonymous>";

/// `vm.Script` 实例的编译数据（Go `scriptData`：source + filename）。
#[derive(Clone)]
struct ScriptRecord {
    source: String,
    #[allow(dead_code)]
    filename: String,
}

// Script 实例注册表（键 = 实例堆句柄下标；Go `vmState.scripts` 的对应物）。
thread_local! {
    static SCRIPTS: RefCell<HashMap<u32, ScriptRecord>> = RefCell::new(HashMap::new());
}

// `Script.prototype` 句柄（build 时暂存，构造实例时拷贝方法为自有属性）。
thread_local! {
    static SCRIPT_PROTO: RefCell<Option<ObjectRef>> = const { RefCell::new(None) };
}

/// `require("vm")` / `require("node:vm")` 主模块。
pub const MODULE: ModuleDef = ModuleDef { name: "vm", build };

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 顶层方法：runInThisContext/createContext/isContext/runInContext/
    // runInNewContext/compileFunction/measureMemory/createScript
    for method in [
        "runInThisContext",
        "createContext",
        "isContext",
        "runInContext",
        "runInNewContext",
        "compileFunction",
        "measureMemory",
        "createScript",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("vm.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }

    // vm.constants：Node 22 空对象
    let constants = vm.alloc_ordinary();
    set_module_prop(vm, obj, "constants", Value::Object(constants))?;

    // Script 原型：四个方法（实例拷贝为自有属性，对齐 Go scriptProto.Keys 拷贝）
    let proto = vm.alloc_ordinary();
    for method in [
        "runInThisContext",
        "runInNewContext",
        "runInContext",
        "createCachedData",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("vm:script.{method}"));
        set_module_prop(vm, proto, method, Value::Object(fn_ref))?;
    }
    let proto_ns = vm.alloc_string(NS_SCRIPT.to_owned());
    set_module_prop(vm, proto, NS_KEY, Value::Object(proto_ns))?;
    SCRIPT_PROTO.with(|slot| *slot.borrow_mut() = Some(proto));

    // Script 构造器（`new vm.Script(code[, options])` 经 do_construct 按名
    // "Script.Script" 分派；NativeCtor 以携带 prototype 属性）
    let script_ctor = vm.alloc_native_ctor("Script.Script", Some(proto));
    set_module_prop(vm, obj, "Script", Value::Object(script_ctor))?;
    // Go NewFunction 默认 length = 0（`vm.Script.length` 实测为 0）
    set_module_prop(vm, script_ctor, "length", Value::Number(0.0))?;

    register_handler(registry, "vm", "createContext", create_context);
    register_handler(registry, "vm", "isContext", is_context);
    register_handler(registry, "vm", "runInContext", run_in_context);
    register_handler(registry, "vm", "runInNewContext", run_in_new_context);
    register_handler(registry, "vm", "runInThisContext", run_in_this_context);
    register_handler(registry, "vm", "compileFunction", compile_function);
    register_handler(registry, "vm", "measureMemory", measure_memory);
    register_handler(registry, "vm", "createScript", create_script);
    register_handler(registry, "Script", "Script", script_new);
    register_handler(
        registry,
        "vm:script",
        "runInThisContext",
        script_run_in_this_context,
    );
    register_handler(
        registry,
        "vm:script",
        "runInNewContext",
        script_run_in_new_context,
    );
    register_handler(registry, "vm:script", "runInContext", script_run_in_context);
    register_handler(
        registry,
        "vm:script",
        "createCachedData",
        script_create_cached_data,
    );
    register_handler(registry, "vm", "compiledFunction", compiled_fn_call);
    Ok(obj)
}

// ---------------------------------------------------------------------------
// context 管理（createContext / isContext）
// ---------------------------------------------------------------------------

/// `vm.createContext([contextObject])`：有 sandbox 返回同一对象（幂等，
/// Go 实测 identity: true），无 sandbox / 非 Ordinary 实参新建普通对象；
/// 两种情况都写 `_builtinNs = "vm:context"` 标记供 `isContext` 判定。
fn create_context(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = match args.first().map(|v| v.case()) {
        Some(v @ ValueCase::Object(_)) if is_ordinary(vm, Value::from(v)) => v,
        _ => ValueCase::Object(vm.alloc_ordinary()),
    };
    mark_ns(vm, Value::from(target), NS_CONTEXT);
    Ok(Value::from(target))
}

/// `vm.isContext(object)`：非对象 / 无 context 标记 → false（Go 实测
/// `isContext({}) === false`、`isContext(42) === false`、缺参 false）。
fn is_context(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let val = args.first().copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(
        own_ns(vm, val).as_deref() == Some(NS_CONTEXT),
    ))
}

// ---------------------------------------------------------------------------
// 源码求值族：本阶段仅表面（见模块头「架构限制」）
// ---------------------------------------------------------------------------

/// `vm.runInContext(code, contextifiedObject[, options])`：缺 context /
/// context 无效的错误消息与 Go 逐字一致（求值前抛出）；context 有效时
/// 进入源码求值路径——本阶段未实现，抛 [`source_exec_error`]。
fn run_in_context(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 || !is_ordinary(vm, args[1]) {
        return Err(error_instance(
            vm,
            "vm.runInContext: contextifiedObject must be an object",
        ));
    }
    if own_ns(vm, args[1]).as_deref() != Some(NS_CONTEXT) {
        return Err(error_instance(
            vm,
            "The argument 'contextifiedObject' is not a vm.Context",
        ));
    }
    Err(source_exec_error(vm, "vm.runInContext"))
}

/// `vm.runInNewContext(code[, contextObject][, options])`：真实求值本阶段
/// 未实现（无 parser/compiler 依赖），抛说明性错误。
fn run_in_new_context(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Err(source_exec_error(vm, "vm.runInNewContext"))
}

/// `vm.runInThisContext(code[, options])`：真实求值本阶段未实现。
fn run_in_this_context(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Err(source_exec_error(vm, "vm.runInThisContext"))
}

/// `vm.compileFunction(code, params[, options])`：返回函数对象（typeof 与
/// Go 一致）；调用该函数即触发源码求值——本阶段未实现，抛说明性错误。
fn compile_function(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let fn_ref = vm.alloc_native_fn("vm.compiledFunction");
    Ok(Value::Object(fn_ref))
}

/// `compileFunction` 产物的调用体：源码求值未实现。
fn compiled_fn_call(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Err(source_exec_error(vm, "vm.compileFunction"))
}

/// `vm.measureMemory()`：返回已兑现 Promise，形状对齐 Go（total/js 两个
/// 计量对象，各含 jsMemoryEstimate 与 jsMemoryRange 数组）；具体字节数
/// 两边均非确定值，测试只对拍形状。
fn measure_memory(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let entry = |vm: &mut Vm| -> Result<ObjectRef, VmError> {
        let o = vm.alloc_ordinary();
        let estimate = Value::Number(0.0);
        set_module_prop(vm, o, "jsMemoryEstimate", estimate)?;
        let range = vm.alloc_array(vec![Value::Number(0.0), Value::Number(0.0)]);
        set_module_prop(vm, o, "jsMemoryRange", Value::Object(range))?;
        Ok(o)
    };
    let js = entry(vm)?;
    let total = entry(vm)?;
    let out = vm.alloc_ordinary();
    set_module_prop(vm, out, "total", Value::Object(total))?;
    set_module_prop(vm, out, "js", Value::Object(js))?;
    let promise = vm.alloc_fulfilled_promise(Value::Object(out));
    Ok(Value::Object(promise))
}

// ---------------------------------------------------------------------------
// vm.Script（构造器 + 原型方法）
// ---------------------------------------------------------------------------

/// `new vm.Script(code[, options])`：记录 source/filename（Go `scriptData`），
/// 拷贝原型方法为自有属性（对齐 Go），传 `cachedData` 时置
/// `cachedDataRejected = true`（Go 同值）。构造期语法校验依赖编译器，
/// 本阶段跳过（已知差异，见模块头）。
fn script_new(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let source = str_arg(vm, args, 0);
    let mut filename = DEFAULT_FILENAME.to_owned();
    let mut has_cached_data = false;
    if let Some(opts) = args.get(1) {
        if is_ordinary(vm, *opts) {
            let f = vm.get_property(*opts, "filename")?;
            if !matches!(f, Value::Undefined) {
                filename = vm.format_value(f);
            }
            let c = vm.get_property(*opts, "cachedData")?;
            if !matches!(c, Value::Undefined | Value::Null) {
                has_cached_data = true;
            }
        }
    }

    let inst = vm.alloc_ordinary();
    let proto = SCRIPT_PROTO.with(|slot| slot.borrow().as_ref().copied());
    if let Some(proto) = proto {
        if matches!(
            vm.heap.get(proto.index()),
            Some(HeapObject::Ordinary { .. })
        ) {
            // 只拷贝方法（NativeFn 属性），排除 _builtinNs 标记
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
    let inst_ns = vm.alloc_string(NS_SCRIPT.to_owned());
    set_module_prop(vm, inst, NS_KEY, Value::Object(inst_ns))?;
    if has_cached_data {
        set_module_prop(vm, inst, "cachedDataRejected", Value::Boolean(true))?;
    }
    SCRIPTS.with(|slot| {
        slot.borrow_mut()
            .insert(inst.0, ScriptRecord { source, filename })
    });
    Ok(Value::Object(inst))
}

/// `vm.createScript(code[, options])`：`new vm.Script` 的废弃别名（DEP0094）。
fn create_script(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    script_new(vm, args)
}

/// 取当前接收者对应的 Script 记录（须带 `"vm:script"` 标记且在注册表内）。
fn script_record(vm: &Vm, this: Value) -> Option<ScriptRecord> {
    if own_ns(vm, this).as_deref() != Some(NS_SCRIPT) {
        return None;
    }
    let ValueCase::Object(r) = this.case() else {
        return None;
    };
    SCRIPTS.with(|slot| slot.borrow().get(&r.0).cloned())
}

/// `script.runInThisContext([options])`：非 Script 实例错误与 Go 逐字一致；
/// 实例真实求值本阶段未实现。
fn script_run_in_this_context(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    match script_record(vm, current_receiver()) {
        None => Err(error_instance(
            vm,
            "vm.Script.runInThisContext: not a Script instance",
        )),
        Some(_) => Err(source_exec_error(vm, "vm.Script.runInThisContext")),
    }
}

/// `script.runInNewContext([contextObject])`：同上，实例求值未实现。
fn script_run_in_new_context(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    match script_record(vm, current_receiver()) {
        None => Err(error_instance(
            vm,
            "vm.Script.runInNewContext: not a Script instance",
        )),
        Some(_) => Err(source_exec_error(vm, "vm.Script.runInNewContext")),
    }
}

/// `script.runInContext(contextifiedObject)`：三级错误消息（非实例 / 缺参 /
/// 非法 context）与 Go 逐字一致；实例 + 合法 context 真实求值未实现。
fn script_run_in_context(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if script_record(vm, current_receiver()).is_none() {
        return Err(error_instance(
            vm,
            "vm.Script.runInContext: not a Script instance",
        ));
    }
    let target = args.first().copied().unwrap_or(Value::Undefined);
    if !is_ordinary(vm, target) {
        return Err(error_instance(
            vm,
            "vm.Script.runInContext: contextifiedObject required",
        ));
    }
    if own_ns(vm, target).as_deref() != Some(NS_CONTEXT) {
        return Err(error_instance(
            vm,
            "The argument 'contextifiedObject' is not a vm.Context",
        ));
    }
    Err(source_exec_error(vm, "vm.Script.runInContext"))
}

/// `script.createCachedData()`：返回源码字节的 Buffer（Go 同为源码字节，
/// 非 V8 缓存格式）；非 Script 实例错误与 Go 逐字一致。
fn script_create_cached_data(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(rec) = script_record(vm, current_receiver()) else {
        return Err(error_instance(
            vm,
            "vm.Script.createCachedData: not a Script instance",
        ));
    };
    Ok(Value::Object(buffer::create_buffer_instance(
        vm,
        rec.source.into_bytes(),
    )))
}

// ---------------------------------------------------------------------------
// 内部辅助
// ---------------------------------------------------------------------------

/// 实参是否为 JS 普通对象（Ordinary 堆对象；字符串/数组等非 Ordinary 值
/// 按 Go `IsObject` 语义排除——字符串在本 VM 中也是堆对象，需特判）。
fn is_ordinary(vm: &Vm, val: Value) -> bool {
    matches!(val.case(), ValueCase::Object(r) if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. }))
    )
}

/// 读对象自有 `_builtinNs` 堆字符串（对齐 `builtins/mod.rs::builtin_ns`，
/// 但只读自有属性、不沿原型链）。
fn own_ns(vm: &Vm, val: Value) -> Option<String> {
    let ValueCase::Object(r) = val.case() else {
        return None;
    };
    if !matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })) {
        return None;
    }
    let s = vm.own_value(r.index(), NS_KEY)?;
    match s.case() {
        ValueCase::Object(sr) => match vm.heap.get(sr.index())? {
            HeapObject::String(text) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// 写 `_builtinNs` 标记（堆字符串属性）。
fn mark_ns(vm: &mut Vm, val: Value, ns: &str) {
    let s = vm.alloc_string(ns.to_owned());
    let _ = vm.set_property(val, NS_KEY, Value::Object(s));
}

/// 文本实参（Go `nodebase.StrArg`）：缺失/null-ish 按空串。
fn str_arg(vm: &Vm, args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(v) if !(v.is_undefined() || v.is_null()) => vm.format_value(*v),
        _ => String::new(),
    }
}

/// 抛 `name: "Error"` 的错误实例（Go 原生函数错误 → JS Error 对象）。
fn error_instance(vm: &mut Vm, message: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_error_instance(message)))
}

/// 源码求值未实现的说明性错误（本阶段架构限制，见模块头）。
fn source_exec_error(vm: &mut Vm, api: &str) -> VmError {
    error_instance(
        vm,
        &format!(
            "{api}: source evaluation is not supported by aluka-vm \
             (aluka-parser/aluka-compiler dependency is forbidden by the ISA contract)"
        ),
    )
}

/// 编译期锚定：确保处理器签名与注册表一致。
/// GC 根快照：`node:vm` 脚本原型单例。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    SCRIPT_PROTO.with(|g| {
        if let Some(r) = g.borrow().as_ref() {
            out.push(Value::Object(*r));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = create_context;
        let _: crate::builtins::BuiltinHandler = is_context;
        let _: crate::builtins::BuiltinHandler = run_in_context;
        let _: crate::builtins::BuiltinHandler = run_in_new_context;
        let _: crate::builtins::BuiltinHandler = run_in_this_context;
        let _: crate::builtins::BuiltinHandler = compile_function;
        let _: crate::builtins::BuiltinHandler = measure_memory;
        let _: crate::builtins::BuiltinHandler = create_script;
        let _: crate::builtins::BuiltinHandler = script_new;
        let _: crate::builtins::BuiltinHandler = script_run_in_this_context;
        let _: crate::builtins::BuiltinHandler = script_run_in_new_context;
        let _: crate::builtins::BuiltinHandler = script_run_in_context;
        let _: crate::builtins::BuiltinHandler = script_create_cached_data;
        let _: crate::builtins::BuiltinHandler = compiled_fn_call;
    }
}
