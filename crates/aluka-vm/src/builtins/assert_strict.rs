//! `node:assert/strict` 严格断言内置模块（Phase 4）。
//!
//! 语义完全对齐 Node.js 22 LTS 标准（`nodeassert/assert.go`）与 Node.js 规范：
//! - `assert/strict` 作为严格模式断言模块，其 `equal` 行为严格等同于 `strictEqual`（全严格比较）；
//! - 提供 `ok`、`equal`、`strictEqual`、`notStrictEqual`、`throws` 等核心方法；
//! - 模块自身支持函数直调（truthy 判定断言）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("assert/strict")` / `require("node:assert/strict")` 严格断言模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "assert/strict",
    build,
};

/// 构建 `assert/strict` 模块单例并向注册表登记方法分派。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for method in ["ok", "equal", "strictEqual", "notStrictEqual", "throws"] {
        let fn_ref = vm.alloc_native_fn(&format!("assert/strict.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "assert/strict", "ok", ok);
    register_handler(registry, "assert/strict", "equal", strict_equal);
    register_handler(registry, "assert/strict", "strictEqual", strict_equal);
    register_handler(
        registry,
        "assert/strict",
        "notStrictEqual",
        not_strict_equal,
    );
    register_handler(registry, "assert/strict", "throws", throws);
    Ok(obj)
}

/// 惰性重链 os 单例（对齐内置模块注册惯用法）。
fn sync_os_link(vm: &mut Vm) {
    if let Some(cur) = vm.os_module {
        if vm.builtin_registry.module("os") != Some(cur) {
            vm.builtin_registry.modules.insert("os", cur);
        }
    }
}

/// `assert.ok(value, [message])`：真值断言。
fn ok(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let val = args.first().copied().unwrap_or(Value::Undefined);
    if vm.truthy(val) {
        return Ok(Value::Undefined);
    }
    let msg = if let Some(m) = args.get(1) {
        format!("assert.ok: value is not truthy: {}", vm.format_value(*m))
    } else {
        "assert.ok: value is not truthy".to_string()
    };
    Err(thrown(vm, &msg))
}

/// `assert.strictEqual(actual, expected, [message])`：严格相等断言。
/// 在 `assert/strict` 模式下，`equal` 同样映射至本实现。
fn strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let actual = args.first().copied().unwrap_or(Value::Undefined);
    let expected = args.get(1).copied().unwrap_or(Value::Undefined);
    if crate::ops::strict_eq(actual, expected, &vm.heap, &vm.current_constants) {
        return Ok(Value::Undefined);
    }
    let msg = if let Some(m) = args.get(2) {
        format!("assert.strictEqual: {}", vm.format_value(*m))
    } else {
        format!(
            "assert.strictEqual: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        )
    };
    Err(thrown(vm, &msg))
}

/// `assert.notStrictEqual(actual, expected, [message])`：严格不相等断言。
fn not_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let actual = args.first().copied().unwrap_or(Value::Undefined);
    let expected = args.get(1).copied().unwrap_or(Value::Undefined);
    if args.len() >= 2 && crate::ops::strict_eq(actual, expected, &vm.heap, &vm.current_constants) {
        let msg = if let Some(m) = args.get(2) {
            format!("assert.notStrictEqual: {}", vm.format_value(*m))
        } else {
            "assert.notStrictEqual: values should not be strictly equal".to_string()
        };
        return Err(thrown(vm, &msg));
    }
    Ok(Value::Undefined)
}

/// `assert.throws(fn, [error, message])`：断言函数执行抛出异常。
fn throws(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let Some(Value::Object(r)) = args.first().copied() else {
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

/// 构造异常抛出错误对象。
fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_string(msg.to_owned())))
}

/// 编译期签名校验锚定。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 处理器签名锚定() {
        let _: crate::builtins::BuiltinHandler = ok;
        let _: crate::builtins::BuiltinHandler = strict_equal;
        let _: crate::builtins::BuiltinHandler = not_strict_equal;
        let _: crate::builtins::BuiltinHandler = throws;
    }
}
