//! `trace_events` 内置模块（Node.js 22 语义，Node.js 22 LTS 标准：
//! Node.js 22 LTS 规范）。
//!
//! - `getEnabledCategories()`：无 CLI 启用分类 → `undefined`；
//! - `createTracing(options)`：`categories` 必须是非空数组，否则抛带
//!   `code` 的 TypeError（`ERR_INVALID_ARG_TYPE` /
//!   `ERR_TRACE_EVENTS_CATEGORY_REQUIRED`，消息与 Go 逐字一致）；
//!   返回 Tracing 对象（`categories` 逗号拼接串、`enabled` 布尔、
//!   `enable()`/`disable()` 切换）。
//!
//! Tracing 实例经 `_builtinNs = "trace_events:tracing"` 标记走
//! `builtin_ns` 实例分派（`mod.rs` 机制），`enable/disable` 写回实例的
//! `enabled` 属性。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// Tracing 实例的命名空间标记（`builtin_ns` 机制）。
const NS_TRACING: &str = "trace_events:tracing";
/// 命名空间标记键。
const NS_KEY: &str = "_builtinNs";

/// `require("trace_events")` / `require("node:trace_events")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "trace_events",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in ["getEnabledCategories", "createTracing"] {
        let fn_ref = vm.alloc_native_fn(&format!("trace_events.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(
        registry,
        "trace_events",
        "getEnabledCategories",
        get_enabled_categories,
    );
    register_handler(registry, "trace_events", "createTracing", create_tracing);
    register_handler(registry, "trace_events:tracing", "enable", tracing_enable);
    register_handler(registry, "trace_events:tracing", "disable", tracing_disable);
    Ok(obj)
}

/// `trace_events.getEnabledCategories()`：无 CLI 启用分类 → `undefined`。
fn get_enabled_categories(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `trace_events.createTracing(options)`：构造 Tracing 对象。
/// `options.categories` 缺失/非数组 → `ERR_INVALID_ARG_TYPE`；空数组 →
/// `ERR_TRACE_EVENTS_CATEGORY_REQUIRED`（消息与 Go 逐字一致）；options
/// 缺席/非对象时跳过校验，`categories` 为空串（Go 同）。
fn create_tracing(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut categories = String::new();
    if let Some(opts) = args.first() {
        if matches!(opts.case(), ValueCase::Object(_)) {
            let c = vm.get_property(*opts, "categories")?;
            let elems: Option<Vec<Value>> = match c.case() {
                ValueCase::Object(r) => match vm.heap.get(r.index()) {
                    Some(HeapObject::Array { elements, .. }) => Some(elements.clone()),
                    _ => None,
                },
                _ => None,
            };
            let Some(elems) = elems else {
                return Err(typed_coded_error(
                    vm,
                    &format!(
                        "The \"options.categories\" property must be an instance of Array. Received type {}",
                        go_type_name(vm, c)
                    ),
                    "ERR_INVALID_ARG_TYPE",
                ));
            };
            if elems.is_empty() {
                return Err(typed_coded_error(
                    vm,
                    "At least one category is required",
                    "ERR_TRACE_EVENTS_CATEGORY_REQUIRED",
                ));
            }
            let parts: Vec<String> = elems.iter().map(|v| vm.format_value(*v)).collect();
            categories = parts.join(",");
        }
    }

    let t = vm.alloc_ordinary();
    let cat_ref = vm.alloc_string(categories);
    set_module_prop(vm, t, "categories", Value::Object(cat_ref))?;
    set_module_prop(vm, t, "enabled", Value::Boolean(false))?;
    let enable_fn = vm.alloc_native_fn("trace_events:tracing.enable");
    set_module_prop(vm, t, "enable", Value::Object(enable_fn))?;
    let disable_fn = vm.alloc_native_fn("trace_events:tracing.disable");
    set_module_prop(vm, t, "disable", Value::Object(disable_fn))?;
    let ns_ref = vm.alloc_string(NS_TRACING.to_owned());
    set_module_prop(vm, t, NS_KEY, Value::Object(ns_ref))?;
    Ok(Value::Object(t))
}

/// `tracing.enable()`：置实例 `enabled = true`。
fn tracing_enable(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let _ = vm.set_property(current_receiver(), "enabled", Value::Boolean(true));
    Ok(Value::Undefined)
}

/// `tracing.disable()`：置实例 `enabled = false`。
fn tracing_disable(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let _ = vm.set_property(current_receiver(), "enabled", Value::Boolean(false));
    Ok(Value::Undefined)
}

/// 值的 Go 类型名（对齐 Go `engine.ValueType.String()`，即 typeof 语义：
/// null → "object"、字符串 → "string"、函数 → "function"）。
fn go_type_name(vm: &Vm, val: Value) -> &'static str {
    match val.case() {
        ValueCase::Undefined => "undefined",
        ValueCase::Null => "object",
        ValueCase::Boolean(_) => "boolean",
        ValueCase::Number(_) => "number",
        ValueCase::Object(r) => match vm.heap.get(r.index()) {
            Some(HeapObject::String(_)) => "string",
            Some(HeapObject::BigInt(_)) => "bigint",
            Some(
                HeapObject::Closure { .. }
                | HeapObject::NativeCtor { .. }
                | HeapObject::NativeFn { .. },
            ) => "function",
            _ => "object",
        },
    }
}

/// 抛带 `code` 的 TypeError 实例（Go `nodebase.NewCodedError` 家族：
/// `e.name = "TypeError"`、`e.message`、`e.code` 均可读）。
fn typed_coded_error(vm: &mut Vm, message: &str, code: &str) -> VmError {
    let obj = vm.alloc_error_instance(message);
    let name = vm.alloc_string("TypeError".to_owned());
    let _ = vm.set_property(Value::Object(obj), "name", Value::Object(name));
    let code_ref = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(obj), "code", Value::Object(code_ref));
    VmError::Thrown(Value::Object(obj))
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = get_enabled_categories;
        let _: crate::builtins::BuiltinHandler = create_tracing;
        let _: crate::builtins::BuiltinHandler = tracing_enable;
        let _: crate::builtins::BuiltinHandler = tracing_disable;
    }
}
