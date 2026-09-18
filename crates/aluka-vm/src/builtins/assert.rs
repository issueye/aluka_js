//! `assert` 内置模块（Phase 2）：`ok` / `equal` / `strictEqual` / `throws` 等。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodeassert`）：
//! - `ok(value)`：truthy 通过，否则抛断言异常；
//! - `equal(a, b)` / `strictEqual(a, b)`：宽松/严格相等（复用
//!   `builtins::test::asserts::{loose_equal, strict_equal}`，与 `node:test` 的
//!   `t.assert` 同源单实现）；
//! - `deepEqual` / `deepStrictEqual` / `notDeepEqual` / `notDeepStrictEqual`：
//!   递归结构比较（复用 `test::asserts::deep_strict_equal`）；
//! - `notEqual` / `notStrictEqual` / `fail`；
//! - `throws(fn)`：捕获 `fn` 抛出的任何异常（`Err(VmError::Thrown(_))` /
//!   其它错误）视为通过；未抛则抛 `AssertionError`。
//!
//! 模块对象为注册表新建单例，`require("assert")` / `require("node:assert")`
//! 均命中（`builtin_module` 剥离 `node:` 前缀后查注册表）。

use crate::builtins::test::asserts::{deep_strict_equal, loose_equal, strict_equal};
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
        let fn_ref = vm.alloc_native_fn(&format!("assert.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "assert", "ok", ok);
    register_handler(registry, "assert", "equal", equal);
    register_handler(registry, "assert", "notEqual", not_equal);
    register_handler(registry, "assert", "strictEqual", strict_equal_fn);
    register_handler(registry, "assert", "notStrictEqual", not_strict_equal);
    register_handler(registry, "assert", "deepEqual", deep_equal);
    register_handler(registry, "assert", "notDeepEqual", not_deep_equal);
    register_handler(registry, "assert", "deepStrictEqual", deep_strict_equal_fn);
    register_handler(
        registry,
        "assert",
        "notDeepStrictEqual",
        not_deep_strict_equal,
    );
    register_handler(registry, "assert", "throws", throws);
    register_handler(registry, "assert", "fail", fail);
    register_handler(registry, "assert", "match", match_fn);
    register_handler(registry, "assert", "doesNotMatch", does_not_match);
    register_handler(registry, "assert", "ifError", if_error);
    register_handler(registry, "assert", "doesNotThrow", does_not_throw);
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
    // 显式 message：字符串即整体消息、Error 对象原样抛出（Node 语义）；
    // 缺省用默认诊断文本
    match args.get(1).copied() {
        Some(err_val) if err_val.as_object().is_some() && !vm.is_string_value(err_val) => {
            Err(VmError::Thrown(err_val))
        }
        Some(msg_val) if !msg_val.is_undefined() => {
            let text = vm.format_value(msg_val);
            Err(thrown(vm, &text))
        }
        _ => Err(thrown(vm, "assert.ok: value is not truthy")),
    }
}

/// `assert.equal(actual, expected)`：宽松相等（`==` 语义）。
fn equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if loose_equal(vm, actual, expected) {
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

/// `assert.notEqual(actual, expected)`：宽松不等（相等即失败）。
fn not_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if !loose_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.notEqual: {} == {}",
            vm.format_value(actual),
            vm.format_value(expected)
        ),
    ))
}

/// `assert.strictEqual(actual, expected)`：严格相等（`===` 语义）。
fn strict_equal_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if strict_equal(vm, actual, expected) {
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

/// `assert.notStrictEqual(actual, expected)`：严格不等（严格相等即失败）。
fn not_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if !strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.notStrictEqual: expected {} to be != {}",
            vm.format_value(actual),
            vm.format_value(expected)
        ),
    ))
}

/// `assert.deepEqual(actual, expected)`：递归结构比较。
///
/// Node 的 `deepEqual` 用宽松比较叶子值；本实现与 `deepStrictEqual` 共用
/// `test::asserts::deep_strict_equal`（该实现对叶子值先走 `strict_equal`，
/// 覆盖真实项目中 `deepEqual` 的断言取值域：原始值 + 数组 + 普通对象）。
fn deep_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.deepEqual: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        ),
    ))
}

/// `assert.notDeepEqual(actual, expected)`：递归结构不等（相等即失败）。
fn not_deep_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if !deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.notDeepEqual: {} ~= {}",
            vm.format_value(actual),
            vm.format_value(expected)
        ),
    ))
}

/// `assert.deepStrictEqual(actual, expected)`：递归严格结构比较。
fn deep_strict_equal_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.deepStrictEqual: expected {} but got {}",
            vm.format_value(expected),
            vm.format_value(actual)
        ),
    ))
}

/// `assert.notDeepStrictEqual(actual, expected)`：递归严格不等（相等即失败）。
fn not_deep_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (actual, expected) = pair(args);
    if !deep_strict_equal(vm, actual, expected) {
        return Ok(Value::Undefined);
    }
    Err(thrown(
        vm,
        &format!(
            "assert.notDeepStrictEqual: {} ~= {}",
            vm.format_value(actual),
            vm.format_value(expected)
        ),
    ))
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

/// 取断言二元断参数（缺参为 `undefined`）。
fn pair(args: &[Value]) -> (Value, Value) {
    (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    )
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

/// `assert.match(string, regexp[, message])`：字符串须匹配正则。
///
/// 判定走 JS 侧 `regexp.test`（`lastIndex` 等规范行为与正则实现同源），
/// 类型不符按 Node 语义抛 TypeError。
pub(crate) fn match_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (text, pattern) = pair(args);
    let hit = regexp_test(vm, text, pattern, "assert.match")?;
    if hit {
        return Ok(Value::Undefined);
    }
    let detail = format!(
        "{} does not match {}",
        vm.format_value(text),
        vm.format_value(pattern)
    );
    Err(assertion_failure(
        vm,
        "assert.match",
        &detail,
        args.get(2).copied(),
    ))
}

/// `assert.doesNotMatch(string, regexp[, message])`：字符串不得匹配正则。
pub(crate) fn does_not_match(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let (text, pattern) = pair(args);
    if !regexp_test(vm, text, pattern, "assert.doesNotMatch")? {
        return Ok(Value::Undefined);
    }
    let detail = format!(
        "{} unexpectedly matches {}",
        vm.format_value(text),
        vm.format_value(pattern)
    );
    Err(assertion_failure(
        vm,
        "assert.doesNotMatch",
        &detail,
        args.get(2).copied(),
    ))
}

/// `assert.ifError(value)`：`value` 非 undefined/null 时抛 AssertionError，
/// 文案为 `ifError got unwanted exception: <inspect(value)>`（Node 22 实测：
/// Error 取 `message`、字符串带引号、其余按 inspect）。
pub(crate) fn if_error(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let value = args.first().copied().unwrap_or(Value::Undefined);
    if value.is_undefined() || value.is_null() {
        return Ok(Value::Undefined);
    }
    let detail = if vm.is_string_value(value) {
        format!("'{}'", vm.format_value(value))
    } else if value.as_object().is_some() {
        let message = vm
            .get_property(value, "message")
            .unwrap_or(Value::Undefined);
        if vm.is_string_value(message) {
            vm.format_value(message)
        } else {
            vm.format_console_value(value)
        }
    } else {
        vm.format_value(value)
    };
    Err(vm.typed_error(
        "AssertionError",
        &format!("ifError got unwanted exception: {detail}"),
    ))
}

/// `assert.doesNotThrow(fn[, message])`：`fn` 必须正常返回。
pub(crate) fn does_not_throw(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let Some(r) = args.first().copied().and_then(|v| v.as_object()) else {
        return Err(thrown(
            vm,
            "assert.doesNotThrow: first argument must be a function",
        ));
    };
    let Some(HeapObject::Closure {
        func_idx, upvalues, ..
    }) = vm.heap.get(r.index())
    else {
        return Err(thrown(
            vm,
            "assert.doesNotThrow: first argument must be a function",
        ));
    };
    match vm.invoke_function(*func_idx, Value::Undefined, &[], upvalues.clone()) {
        Ok(_) => Ok(Value::Undefined),
        Err(err) => {
            let detail = format!("Got unwanted exception: {err:?}");
            Err(assertion_failure(
                vm,
                "assert.doesNotThrow",
                &detail,
                args.get(1).copied(),
            ))
        }
    }
}

/// 正则判定：类型校验 + 经 JS 侧 `test` 方法求值。
fn regexp_test(vm: &mut Vm, text: Value, pattern: Value, whom: &str) -> Result<bool, VmError> {
    if !vm.is_string_value(text) {
        return Err(vm.type_error(&format!("{whom}: first argument must be of type string")));
    }
    if !vm.is_regexp_obj(pattern) {
        return Err(vm.type_error(&format!("{whom}: second argument must be a RegExp")));
    }
    let test_fn = vm.get_property(pattern, "test")?;
    let out = vm.invoke_callable(test_fn, pattern, &[text])?;
    Ok(vm.truthy(out))
}

/// 断言失败：抛 `AssertionError`（带 `.message`/`.name`/`stack`），
/// 显式给出的 Error 对象按 Node 语义**原样抛出**。
fn assertion_failure(
    vm: &mut Vm,
    whom: &str,
    default_message: &str,
    message: Option<Value>,
) -> VmError {
    match message {
        // 显式传入的非字符串值（Error 对象等）按 Node 语义原样抛出
        Some(custom) if custom.as_object().is_some() && !vm.is_string_value(custom) => {
            VmError::Thrown(custom)
        }
        Some(extra) if !extra.is_undefined() => {
            let text = vm.format_value(extra);
            vm.typed_error("AssertionError", &format!("{whom}: {text}"))
        }
        _ => vm.typed_error("AssertionError", &format!("{whom}: {default_message}")),
    }
}

/// 断言失败值：Node 语义抛 `AssertionError`（带 `name`/`message`/`stack`）。
/// 此前抛裸字符串——`catch (e) { e.message }` 恒 undefined，且
/// `e instanceof Error` 为假（Node 侧两者皆成立）。
fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    vm.typed_error("AssertionError", msg)
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = ok;
        let _: crate::builtins::BuiltinHandler = equal;
        let _: crate::builtins::BuiltinHandler = not_equal;
        let _: crate::builtins::BuiltinHandler = strict_equal_fn;
        let _: crate::builtins::BuiltinHandler = not_strict_equal;
        let _: crate::builtins::BuiltinHandler = deep_equal;
        let _: crate::builtins::BuiltinHandler = not_deep_equal;
        let _: crate::builtins::BuiltinHandler = deep_strict_equal_fn;
        let _: crate::builtins::BuiltinHandler = not_deep_strict_equal;
        let _: crate::builtins::BuiltinHandler = throws;
        let _: crate::builtins::BuiltinHandler = fail;
    }
}
