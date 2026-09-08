//! `repl` 内置模块（Phase 7）：Node 22 交互解释环境。
//!
//! 语义严格对齐 Node.js 22 LTS 规范：
//! - `start({ prompt, eval })` 启动阻塞式读行-求值循环：逐行打印提示符、读
//!   stdin，空行跳过，`.exit`/`exit` 退出，stdin EOF 退出（提示符后直接结束）；
//! - 自定义 `eval(cmd, context, file, cb)`：`cb(null, result)` 且 result 非
//!   `undefined` 时打印（对齐 Go `fmt.Println`）；
//! - REPLServer 方法面：`setPrompt` / `getPrompt` / `displayPrompt` /
//!   `defineCommand` / `clearBufferedCommand` / `setupHistory` / `close` 与
//!   `context` 属性。
//!
//! 已知限制：Go 版默认 eval 分支解引用 nil 选项即 panic（无观测行为），
//! Rust 侧无 eval 选项时静默消费输入行（不挂起、不崩溃），见汇报。

use crate::builtins::readline::{is_callable_value, print_direct, read_line_shared};
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as IoWrite;

/// `require("repl")` / `require("node:repl")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "repl",
    build,
};

/// REPLServer 实例的会话状态（键为实例堆句柄索引）。
struct ReplState {
    /// 当前提示符（`setPrompt` 可改）
    prompt: String,
}

// REPLServer 实例状态表：实例句柄 → 会话状态。
thread_local! {
    // REPL_STATES：线程局部（堆句柄仅本线程 Vm 有效）。
    static REPL_STATES: RefCell<Option<HashMap<u32, ReplState>>> = const { RefCell::new(None) };
}

/// 构建 `repl` 模块对象并登记分派处理器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let start_fn = vm.alloc_native_fn("repl.start");
    set_module_prop(vm, obj, "start", Value::Object(start_fn))?;
    register_handler(registry, "repl", "start", repl_start);

    for method in [
        "setPrompt",
        "getPrompt",
        "displayPrompt",
        "defineCommand",
        "clearBufferedCommand",
        "setupHistory",
        "close",
        "callback",
    ] {
        register_handler(registry, "repl:server", method, repl_server_method(method));
    }
    Ok(obj)
}

/// REPLServer 方法名 → 处理器（编译期展开为独立 fn 指针）。
fn repl_server_method(method: &'static str) -> crate::builtins::BuiltinHandler {
    match method {
        "setPrompt" => server_set_prompt,
        "getPrompt" => server_get_prompt,
        "displayPrompt" => server_display_prompt,
        "defineCommand" => server_define_command,
        "clearBufferedCommand" => server_clear_buffered_command,
        "setupHistory" => server_setup_history,
        "close" => server_close,
        "callback" => server_callback,
        _ => server_define_command,
    }
}

/// `repl.start(options)`：启动阻塞式 REPL 循环，结束后返回 REPLServer 实例。
fn repl_start(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut prompt = "> ".to_owned();
    let mut eval_fn: Option<Value> = None;
    if let Some(Value::Object(o)) = args.first().copied() {
        if let Ok(v) = vm.get_property(Value::Object(o), "prompt") {
            if !matches!(v, Value::Undefined) {
                prompt = vm.format_value(v);
            }
        }
        if let Ok(v) = vm.get_property(Value::Object(o), "eval") {
            if is_callable_value(vm, v) {
                eval_fn = Some(v);
            }
        }
    }

    let repl_obj = vm.alloc_ordinary();
    let ns = Value::Object(vm.alloc_string("repl:server".to_owned()));
    set_module_prop(vm, repl_obj, "_builtinNs", ns)?;
    let id = repl_obj.0;
    REPL_STATES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, ReplState { prompt });
    });

    for method in [
        "setPrompt",
        "getPrompt",
        "displayPrompt",
        "defineCommand",
        "clearBufferedCommand",
        "setupHistory",
        "close",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("repl:server.{method}"));
        set_module_prop(vm, repl_obj, method, Value::Object(fn_ref))?;
    }
    // context 属性：全局上下文对象（Rust VM 以独立上下文对象近似 Go 的 global）。
    let context = vm.alloc_ordinary();
    set_module_prop(vm, repl_obj, "context", Value::Object(context))?;

    // 主循环（阻塞；CLI 交互场景运行）。stdin EOF 时退出，不额外换行。
    loop {
        print_direct(&state_prompt(id));
        let Some(raw) = read_line_shared() else {
            break;
        };
        let line = raw.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        if line == ".exit" || line == "exit" {
            break;
        }
        let Some(eval) = eval_fn else {
            // Go 版默认 eval 分支解引用 nil 选项即 panic；此处静默消费该行。
            continue;
        };
        let cb = Value::Object(vm.alloc_native_fn("repl:server.callback"));
        let cmd = Value::Object(vm.alloc_string(line));
        let file = Value::Object(vm.alloc_string("[repl]".to_owned()));
        vm.invoke_callable(
            eval,
            Value::Undefined,
            &[cmd, Value::Object(context), file, cb],
        )?;
    }
    Ok(Value::Object(repl_obj))
}

/// 自定义 eval 的回调 `cb(null, result)`：result 非 `undefined` 时打印。
fn server_callback(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(v) = args.get(1) {
        if !matches!(v, Value::Undefined) {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", vm.format_value(*v));
            let _ = out.flush();
        }
    }
    Ok(Value::Undefined)
}

/// `server.setPrompt(prompt)`：更新提示符（链式返回实例）。
fn server_set_prompt(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    if let Some(v) = args.first() {
        set_state_prompt(r.0, &vm.format_value(*v));
    }
    Ok(receiver)
}

/// `server.getPrompt()`：读取当前提示符。
fn server_get_prompt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => 0,
    };
    Ok(Value::Object(vm.alloc_string(state_prompt(id))))
}

/// `server.displayPrompt()`：输出当前提示符（链式返回实例）。
fn server_display_prompt(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => 0,
    };
    print_direct(&state_prompt(id));
    Ok(receiver)
}

/// `server.defineCommand(...)`：no-op（返回 `undefined`）。
fn server_define_command(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `server.clearBufferedCommand()`：链式返回实例。
fn server_clear_buffered_command(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `server.setupHistory(...)`：no-op（返回 `undefined`）。
fn server_setup_history(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `server.close()`：链式返回实例。
fn server_close(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// 读取实例提示符（未登记时为空串）。
fn state_prompt(id: u32) -> String {
    REPL_STATES.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&id))
            .map(|s| s.prompt.clone())
            .unwrap_or_default()
    })
}

/// 更新实例提示符。
fn set_state_prompt(id: u32, prompt: &str) {
    REPL_STATES.with(|g| {
        if let Some(m) = g.borrow_mut().as_mut() {
            if let Some(s) = m.get_mut(&id) {
                s.prompt = prompt.to_owned();
            }
        }
    });
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
        let _: crate::builtins::BuiltinHandler = repl_start;
        let _: crate::builtins::BuiltinHandler = server_callback;
        let _: crate::builtins::BuiltinHandler = server_set_prompt;
        let _: crate::builtins::BuiltinHandler = server_get_prompt;
        let _: crate::builtins::BuiltinHandler = server_display_prompt;
        let _: crate::builtins::BuiltinHandler = server_define_command;
        let _: crate::builtins::BuiltinHandler = server_clear_buffered_command;
        let _: crate::builtins::BuiltinHandler = server_setup_history;
        let _: crate::builtins::BuiltinHandler = server_close;
    }
}
