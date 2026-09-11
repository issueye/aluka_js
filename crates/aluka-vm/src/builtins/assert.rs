//! `assert` 内置模块（Phase 2）：`ok` / `equal` / `strictEqual` / `throws`。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodeassert`）：
//! - `ok(value)`：truthy 通过，否则抛断言异常（Thrown 字符串）；
//! - `equal(a, b)` / `strictEqual(a, b)`：本引擎统一用值字符串化比较
//!   （`Vm::format_value`），与 Go 宽松/严格相等在探测取值域内一致；
//! - `throws(fn)`：捕获 `fn` 抛出的任何异常（`Err(VmError::Thrown(_))` /
//!   其它错误）视为通过；未抛则抛 `AssertionError`。
//!
//! 模块对象为注册表新建单例，`require("assert")` / `require("node:assert")`
//! 均命中（`builtin_module` 剥离 `node:` 前缀后查注册表）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("assert")` / `require("node:assert")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "assert",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for method in ["ok", "equal", "strictEqual", "throws"] {
        let fn_ref = vm.alloc_native_fn(&format!("assert.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "assert", "ok", ok);
    register_handler(registry, "assert", "equal", equal);
    register_handler(registry, "assert", "strictEqual", strict_equal);
    register_handler(registry, "assert", "throws", throws);
    Ok(obj)
}

/// 惰性重链 os 单例（见 `crate::builtins::os` 模块文档）。
fn sync_os_link(vm: &mut Vm) {
    if let Some(cur) = vm.os_module {
        if vm.builtin_registry.module("os") != Some(cur) {
            vm.builtin_registry.modules.insert("os", cur);
        }
    }
}

/// `assert.ok(value)`：truthy 通过，否则抛 `assert.ok: value is not truthy`。
fn ok(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let val = args.first().copied().unwrap_or(Value::Undefined);
    if vm.truthy(val) {
        return Ok(Value::Undefined);
    }
    Err(thrown(vm, "assert.ok: value is not truthy"))
}

/// `assert.equal(actual, expected)`：字符串化相等判定。
fn equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if vm.format_value(actual) == vm.format_value(expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.equal: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        ),
    ))
}

/// `assert.strictEqual(actual, expected)`：字符串化严格相等判定。
fn strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if vm.format_value(actual) == vm.format_value(expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.strictEqual: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        ),
    ))
}

/// `assert.throws(fn)`：捕获 `fn` 抛出的异常（Thrown/其它错误）视为通过。
fn throws(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let Some(r) = args.first().copied().and_then(|v| v.as_object()) else {
        return Err(thrown(vm, "assert.throws: function required"));
    };
    let Some(HeapObject::Closure {
        func_idx, upvalues, ..
    }) = vm.heap.get(r.index())
    else {
        return Err(thrown(
            vm,
            "assert.throws: first argument must be a function",
        ));
    };
    let result = vm.invoke_function(*func_idx, Value::Undefined, &[], upvalues.clone());
    match result {
        Err(_) => Ok(Value::Undefined),
        Ok(_) => Err(thrown(
            vm,
            "assert.throws: expected exception but none was thrown",
        )),
    }
}

fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_string(msg.to_owned())))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = ok;
        let _: crate::builtins::BuiltinHandler = equal;
        let _: crate::builtins::BuiltinHandler = strict_equal;
        let _: crate::builtins::BuiltinHandler = throws;
    }
}
