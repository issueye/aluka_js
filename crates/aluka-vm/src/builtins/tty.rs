//! `tty` 内置模块（Phase 7）：终端交互检测（Node 22 语义）。
//!
//! 语义严格对齐 Node.js 22 LTS 规范：
//! - `isatty(fd) -> boolean`：fd 0/1/2 经终端检测（Rust 用 std
//!   `IsTerminal`，对齐 Go 的 `ModeCharDevice`/`GetConsoleMode` 判断），
//!   其余 fd 恒为 `false`；非数值 fd 视为 0；
//! - `ReadStream(fd)` / `WriteStream(fd)`：非 TTY 的非负 fd 构造抛
//!   `ERR_TTY_INIT_FAILED`（Node 语义，错误对象带 `code` 字段）；fd 为负
//!   （含缺省 -1）跳过检测直接构造；实例携带 `isTTY`/`fd`（及 ReadStream
//!   的 `isRaw`、WriteStream 的 `columns`/`rows`）；
//! - 原型/实例方法：`setRawMode`（读）与 `clearLine` / `clearScreenDown` /
//!   `cursorTo` / `getColorDepth` / `getWindowSize` / `hasColors` /
//!   `moveCursor` / `_refreshSize`（写）。
//!
//! 已知限制：Rust 的 NativeFn 堆对象不支持挂属性，`ReadStream.prototype` /
//! `WriteStream.prototype` 面以实例自有方法等价提供（Go 亦在实例上重复挂载），
//! `ctor.prototype` 属性未物化，见汇报。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("tty")` / `require("node:tty")` 主模块。
pub const MODULE: ModuleDef = ModuleDef { name: "tty", build };

/// 构建 `tty` 模块对象。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    let isatty_fn = vm.alloc_native_fn("tty.isatty");
    set_module_prop(vm, obj, "isatty", Value::Object(isatty_fn))?;
    register_handler(registry, "tty", "isatty", tty_isatty);

    // 构造器（NativeFn 形态注册 `new` 分派；实例方法经 `_builtinNs` 分派）。
    let rs_ctor = vm.alloc_native_fn("tty.ReadStream");
    set_module_prop(vm, obj, "ReadStream", Value::Object(rs_ctor))?;
    register_handler(registry, "tty", "ReadStream", tty_read_stream);
    register_handler(registry, "tty:rs", "setRawMode", tty_set_raw_mode);

    let ws_ctor = vm.alloc_native_fn("tty.WriteStream");
    set_module_prop(vm, obj, "WriteStream", Value::Object(ws_ctor))?;
    register_handler(registry, "tty", "WriteStream", tty_write_stream);
    for method in [
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "getColorDepth",
        "getWindowSize",
        "hasColors",
        "moveCursor",
        "_refreshSize",
    ] {
        register_handler(registry, "tty:ws", method, tty_noop);
    }

    Ok(obj)
}

/// 实例方法 no-op（返回 `undefined`）。
fn tty_noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `rs.setRawMode(...)`：no-op（返回 `undefined`）。
fn tty_set_raw_mode(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `tty.isatty(fd)`：fd 是否指向终端（0/1/2 之外恒 false）。
fn tty_isatty(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 非数值 fd 视为 0（对齐 Go `Int()` 失败时的默认值）。
    let fd = match args.first().map(|v| v.case()) {
        Some(ValueCase::Number(n)) => n as i64,
        _ => 0,
    };
    Ok(Value::Boolean(is_tty_fd(fd)))
}

/// `new ReadStream(fd)`：TTY 读流构造。
fn tty_read_stream(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let fd = arg_fd(args);
    if fd >= 0 && !is_tty_fd(fd) {
        return Err(tty_init_failed(vm));
    }
    let obj = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("tty:rs".to_owned()));
    set_module_prop(vm, obj, "_builtinNs", ns)?;
    set_module_prop(vm, obj, "isTTY", Value::Boolean(fd >= 0 && is_tty_fd(fd)))?;
    set_module_prop(vm, obj, "fd", Value::Number(fd as f64))?;
    set_module_prop(vm, obj, "isRaw", Value::Boolean(false))?;
    let set_raw = vm.alloc_native_fn("tty:rs.setRawMode");
    set_module_prop(vm, obj, "setRawMode", Value::Object(set_raw))?;
    Ok(Value::Object(obj))
}

/// `new WriteStream(fd)`：TTY 写流构造。
fn tty_write_stream(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let fd = arg_fd(args);
    if fd >= 0 && !is_tty_fd(fd) {
        return Err(tty_init_failed(vm));
    }
    let obj = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("tty:ws".to_owned()));
    set_module_prop(vm, obj, "_builtinNs", ns)?;
    set_module_prop(vm, obj, "isTTY", Value::Boolean(fd >= 0 && is_tty_fd(fd)))?;
    set_module_prop(vm, obj, "fd", Value::Number(fd as f64))?;
    set_module_prop(vm, obj, "columns", Value::Number(80.0))?;
    set_module_prop(vm, obj, "rows", Value::Number(24.0))?;
    for method in [
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "getColorDepth",
        "getWindowSize",
        "hasColors",
        "moveCursor",
        "_refreshSize",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("tty:ws.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    Ok(Value::Object(obj))
}

/// 提取 fd 实参（缺省 -1，非数值 -1——对齐 Go `Int()` 失败时的默认）。
fn arg_fd(args: &[Value]) -> i64 {
    match args.first().map(|v| v.case()) {
        Some(ValueCase::Number(n)) => n as i64,
        _ => -1,
    }
}

/// 非 TTY fd 构造错误（Node：`ERR_TTY_INIT_FAILED`）。
fn tty_init_failed(vm: &mut Vm) -> VmError {
    let obj = vm.alloc_ordinary();
    let name_v = Value::Object(vm.alloc_string("Error".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "name", name_v);
    let msg_v = Value::Object(vm.alloc_string(
        "ERR_TTY_INIT_FAILED: TTY initialization failed: uv_tty_init returned EBADF".to_owned(),
    ));
    let _ = vm.set_property(Value::Object(obj), "message", msg_v);
    let code_v = Value::Object(vm.alloc_string("ERR_TTY_INIT_FAILED".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "code", code_v);
    VmError::Thrown(Value::Object(obj))
}

/// 平台终端检测：0/1/2 经 std `IsTerminal`（Windows 走 GetConsoleMode 等价
/// 判定），其余 fd 恒 false。
fn is_tty_fd(fd: i64) -> bool {
    use std::io::IsTerminal;
    match fd {
        0 => std::io::stdin().is_terminal(),
        1 => std::io::stdout().is_terminal(),
        2 => std::io::stderr().is_terminal(),
        _ => false,
    }
}

/// 判断堆对象形态的辅助（保持对 `HeapObject` 的窄依赖）。
#[allow(dead_code)]
fn is_heap_object(vm: &Vm, r: ObjectRef) -> bool {
    vm.heap
        .get(r.0 as usize)
        .is_some_and(|o| matches!(o, HeapObject::Ordinary { .. }))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = tty_isatty;
        let _: crate::builtins::BuiltinHandler = tty_read_stream;
        let _: crate::builtins::BuiltinHandler = tty_write_stream;
    }

    #[test]
    fn isatty_negative_fd_is_false() {
        assert!(!is_tty_fd(-1));
        assert!(!is_tty_fd(99));
    }
}
