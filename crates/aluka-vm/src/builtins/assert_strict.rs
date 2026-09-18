//! `node:assert/strict` 严格断言内置模块（Phase 4）。
//!
//! 语义完全对齐 Node.js 22 LTS 标准（`nodeassert/assert.go`）与 Node.js 规范：
//! - `assert/strict` 作为严格模式断言模块，其 `equal` 行为严格等同于 `strictEqual`（全严格比较）；
//! - 提供 `ok`、`equal`、`strictEqual`、`notStrictEqual`、`throws` 等核心方法；
//! - 模块自身支持函数直调（truthy 判定断言）。

use crate::builtins::assert::{does_not_match, does_not_throw, if_error, match_fn};
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
///
/// `assert/strict` 是「全部严格」变体：`equal` == `strictEqual`、
/// `deepEqual` == `deepStrictEqual`（Node 语义），两者共用同一处理器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for method in [
        "ok",
        "equal",
        "notEqual",
        "strictEqual",
        "notStrictEqual",
        "deepEqual",
        "notDeepEqual",
        "deepStrictEqual",
        "notDeepStrictEqual",
        "throws",
        "fail",
        "match",
        "doesNotMatch",
        "ifError",
        "doesNotThrow",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("assert/strict.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "assert/strict", "ok", ok);
    register_handler(registry, "assert/strict", "equal", strict_equal);
    register_handler(registry, "assert/strict", "strictEqual", strict_equal);
    register_handler(registry, "assert/strict", "notEqual", not_strict_equal);
    register_handler(
        registry,
        "assert/strict",
        "notStrictEqual",
        not_strict_equal,
    );
    register_handler(registry, "assert/strict", "deepEqual", deep_strict_equal_fn);
    register_handler(
        registry,
        "assert/strict",
        "deepStrictEqual",
        deep_strict_equal_fn,
    );
    register_handler(
        registry,
        "assert/strict",
        "notDeepEqual",
        not_deep_strict_equal,
    );
    register_handler(
        registry,
        "assert/strict",
        "notDeepStrictEqual",
        not_deep_strict_equal,
    );
    register_handler(registry, "assert/strict", "throws", throws);
    register_handler(registry, "assert/strict", "fail", fail);
    // `assert/strict` 的判定面与 `assert` 同源（Node：strict 变体的区别在
    // equal/deepEqual 的严格性，其余方法逐一同源）
    register_handler(registry, "assert/strict", "match", match_fn);
    register_handler(registry, "assert/strict", "doesNotMatch", does_not_match);
    register_handler(registry, "assert/strict", "ifError", if_error);
    register_handler(registry, "assert/strict", "doesNotThrow", does_not_throw);
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
    // 显式 message：字符串即整体消息、Error 对象原样抛出（Node 语义）
    match args.get(1).copied() {
        Some(err_val) if err_val.as_object().is_some() && !vm.is_string_value(err_val) => {
            return Err(VmError::Thrown(err_val));
        }
        Some(msg_val) if !msg_val.is_undefined() => {
            let text = vm.format_value(msg_val);
            return Err(thrown(vm, &text));
        }
        _ => {}
    }
    let msg = "assert.ok: value is not truthy".to_string();
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

/// `assert.deepStrictEqual(actual, expected, [message])`：递归严格结构比较
/// （`assert/strict` 下 `deepEqual` 亦映射至此）。
fn deep_strict_equal_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let actual = args.first().copied().unwrap_or(Value::Undefined);
    let expected = args.get(1).copied().unwrap_or(Value::Undefined);
    if crate::builtins::test::asserts::deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    let msg = if let Some(m) = args.get(2) {
        format!("assert.deepStrictEqual: {}", vm.format_value(*m))
    } else {
        format!(
            "assert.deepStrictEqual: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        )
    };
    Err(thrown(vm, &msg))
}

/// `assert.notDeepStrictEqual(actual, expected, [message])`：递归严格不等
/// （`assert/strict` 下 `notDeepEqual` 亦映射至此）。
fn not_deep_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let actual = args.first().copied().unwrap_or(Value::Undefined);
    let expected = args.get(1).copied().unwrap_or(Value::Undefined);
    if !crate::builtins::test::asserts::deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    let msg = if let Some(m) = args.get(2) {
        format!("assert.notDeepStrictEqual: {}", vm.format_value(*m))
    } else {
        "assert.notDeepStrictEqual: values should not be deeply strictly equal".to_string()
    };
    Err(thrown(vm, &msg))
}

/// `assert.fail([message])`：无条件失败。
fn fail(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let msg = args
        .first()
        .filter(|v| **v != Value::Undefined)
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "Failed".to_owned());
    Err(thrown(vm, &msg))
}

/// `assert.throws(fn, [error, message])`：断言函数执行抛出异常。
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

/// 构造异常抛出错误对象。
/// 断言失败值：与 `assert` 同源，抛 `AssertionError`（Node 语义）。
fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    vm.typed_error("AssertionError", msg)
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
        let _: crate::builtins::BuiltinHandler = deep_strict_equal_fn;
        let _: crate::builtins::BuiltinHandler = not_deep_strict_equal;
        let _: crate::builtins::BuiltinHandler = throws;
        let _: crate::builtins::BuiltinHandler = fail;
    }
}
