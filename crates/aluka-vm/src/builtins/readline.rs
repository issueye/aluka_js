//! `readline` 内置模块（Phase 7）：Node 22 交互式逐行读取（回调版）。
//!
//! 语义严格对齐 Node.js 22 LTS 规范：
//! - 顶层终端工具函数（差分环境无 TTY，全部 no-op）：`emitKeypressEvents`、
//!   `clearLine`、`clearScreenDown`、`cursorTo`、`moveCursor`；
//! - `createInterface({ input, output, terminal })` 返回 EventEmitter 形态的
//!   Interface 实例：`question(query, cb)` 打印提示并阻塞读 stdin 一行——
//!   EOF 时触发 `'close'`（不调回调），成功时置 `rl.line`、先调 `cb(line)`
//!   再触发 `'line'` 事件；
//! - 方法面：`setPrompt` / `getPrompt` / `prompt` / `write` / `getCursorPos` /
//!   `pause` / `resume` / `close`（pause/resume/close 链式返回实例）。
//!
//! 提示符写入路由：`options.output.write(fn)` 存在则写入该流，否则直接写
//! stdout（对齐 Go `fmt.Print` 回退）。

use crate::builtins::events::create_emitter_instance;
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as IoRead;
use std::io::Write as IoWrite;

/// `require("readline")` / `require("node:readline")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "readline",
    build,
};

/// Interface 实例的会话状态（键为实例堆句柄索引）。
struct RlState {
    /// 提示符输出流（`options.output`；无则写 stdout）
    output: Option<Value>,
    /// 当前提示符（`setPrompt` 可改）
    prompt: String,
}

// Interface 实例状态表：实例句柄 → 会话状态。
thread_local! {
    // RL_STATES：线程局部（堆句柄仅本线程 Vm 有效）。
    static RL_STATES: RefCell<Option<HashMap<u32, RlState>>> = const { RefCell::new(None) };
}

/// 构建 `readline` 模块对象并登记全部分派处理器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in [
        "emitKeypressEvents",
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "moveCursor",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("readline.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
        register_handler(registry, "readline", method, noop);
    }

    let create_fn = vm.alloc_native_fn("readline.createInterface");
    set_module_prop(vm, obj, "createInterface", Value::Object(create_fn))?;
    register_handler(registry, "readline", "createInterface", create_interface);

    // Interface 实例方法（实例 `_builtinNs` = "readline:iface"）。
    for method in [
        "question",
        "setPrompt",
        "getPrompt",
        "prompt",
        "write",
        "getCursorPos",
        "pause",
        "resume",
        "close",
    ] {
        register_handler(
            registry,
            "readline:iface",
            method,
            interface_method_dispatch(method),
        );
    }
    // 事件方法转发：Interface 是 EventEmitter，`rl.on/emit/...` 经实例命名
    // 空间命中，转发到 `events:instance` 的既有处理器（复用事件状态）。
    for method in [
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "removeAllListeners",
        "listenerCount",
        "setMaxListeners",
        "getMaxListeners",
        "prependListener",
        "prependOnceListener",
        "eventNames",
        "listeners",
        "rawListeners",
    ] {
        if let Some(h) = registry.lookup(&format!("events:instance.{method}")) {
            register_handler(registry, "readline:iface", method, h);
        }
    }
    Ok(obj)
}

/// Interface 实例方法名 → 处理器（编译期展开为独立 fn 指针）。
fn interface_method_dispatch(method: &'static str) -> crate::builtins::BuiltinHandler {
    match method {
        "question" => interface_question,
        "setPrompt" => interface_set_prompt,
        "getPrompt" => interface_get_prompt,
        "prompt" => interface_prompt,
        "write" => interface_write,
        "getCursorPos" => interface_get_cursor_pos,
        "pause" => interface_pause,
        "resume" => interface_resume,
        "close" => interface_close,
        _ => noop,
    }
}

/// 顶层 no-op 工具函数（返回 `undefined`）。
fn noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `createInterface(options)`：构造 Interface 实例（EventEmitter 形态）。
fn create_interface(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let rl = create_emitter_instance(vm);

    let mut output: Option<Value> = None;
    let mut terminal = true;
    if let Some(o) = args.first().copied().and_then(|v| v.as_object()).map(ValueCase::from) {
        if let Ok(v) = vm.get_property(Value::Object(o), "output") {
            if !matches!(v, Value::Undefined) {
                output = Some(v);
            }
        }
        if let Ok(ValueCase::Boolean(b)) = vm.get_property(Value::Object(o), "terminal").map(ValueCase::from) {
            terminal = b;
        }
    }

    // 实例命名空间：CALL_METHOD 经 `_builtinNs` 命中本模块分派（事件方法
    // 由 build 期从 `events:instance` 转发，见 `build`）。
    let ns = Value::Object(vm.alloc_string("readline:iface".to_owned()));
    set_module_prop(vm, rl, "_builtinNs", ns)?;
    set_module_prop(vm, rl, "terminal", Value::Boolean(terminal))?;
    let empty = Value::Object(vm.alloc_string(String::new()));
    set_module_prop(vm, rl, "line", empty)?;

    let id = rl.0;
    RL_STATES.with(|g| {
        g.borrow_mut().get_or_insert_with(HashMap::new).insert(
            id,
            RlState {
                output,
                prompt: String::new(),
            },
        );
    });

    for method in [
        "question",
        "setPrompt",
        "getPrompt",
        "prompt",
        "write",
        "getCursorPos",
        "pause",
        "resume",
        "close",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("readline:iface.{method}"));
        set_module_prop(vm, rl, method, Value::Object(fn_ref))?;
    }
    Ok(Value::Object(rl))
}

/// 读取当前 Interface 实例状态的提示符文本。
fn state_prompt(id: u32) -> String {
    RL_STATES.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&id))
            .map(|s| s.prompt.clone())
            .unwrap_or_default()
    })
}

/// 读取当前 Interface 实例的输出流。
fn state_output(id: u32) -> Option<Value> {
    RL_STATES.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&id))
            .and_then(|s| s.output)
    })
}

/// 更新当前 Interface 实例的提示符文本。
fn set_state_prompt(id: u32, prompt: &str) {
    RL_STATES.with(|g| {
        if let Some(m) = g.borrow_mut().as_mut() {
            if let Some(s) = m.get_mut(&id) {
                s.prompt = prompt.to_owned();
            }
        }
    });
}

/// `rl.question(query, cb)`：打印提示并阻塞读 stdin 一行。
fn interface_question(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    let query = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let callback = args.get(1).copied().filter(|v| is_callable_value(vm, *v));

    write_prompt(vm, state_output(r.0), &query)?;

    let Some(raw) = read_line_blocking() else {
        // EOF（或无换行的残尾）：触发 'close'，不调用回调（对齐 Go）。
        emit_event(vm, receiver, "close", &[])?;
        return Ok(receiver);
    };
    let line = raw.trim_end_matches(['\r', '\n']).to_owned();
    let line_v = Value::Object(vm.alloc_string(line.clone()));
    set_module_prop(vm, r, "line", line_v)?;
    if let Some(cb) = callback {
        let arg = Value::Object(vm.alloc_string(line.clone()));
        vm.invoke_callable(cb, receiver, &[arg])?;
    }
    let ev_arg = Value::Object(vm.alloc_string(line));
    emit_event(vm, receiver, "line", &[ev_arg])?;
    Ok(receiver)
}

/// `rl.setPrompt(prompt)`：更新提示符（链式返回实例）。
fn interface_set_prompt(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(receiver);
    };
    if let Some(v) = args.first() {
        set_state_prompt(r.0, &vm.format_value(*v));
    }
    Ok(receiver)
}

/// `rl.getPrompt()`：读取当前提示符。
fn interface_get_prompt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver.case() {
        ValueCase::Object(r) => r.0,
        _ => 0,
    };
    Ok(Value::Object(vm.alloc_string(state_prompt(id))))
}

/// `rl.prompt()`：输出当前提示符（不读行）。
fn interface_prompt(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver.case() {
        ValueCase::Object(r) => r.0,
        _ => 0,
    };
    write_prompt(vm, state_output(id), &state_prompt(id))?;
    Ok(Value::Undefined)
}

/// `rl.write(...)`：no-op（对齐 Go 差分环境）。
fn interface_write(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `rl.getCursorPos()`：固定 `{ rows: 0, cols: 0 }`。
fn interface_get_cursor_pos(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let pos = vm.alloc_ordinary();
    set_module_prop(vm, pos, "rows", Value::Number(0.0))?;
    set_module_prop(vm, pos, "cols", Value::Number(0.0))?;
    Ok(Value::Object(pos))
}

/// `rl.pause()`：no-op 链式返回实例。
fn interface_pause(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `rl.resume()`：no-op 链式返回实例。
fn interface_resume(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `rl.close()`：触发 `'close'` 并返回实例。
fn interface_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    emit_event(vm, receiver, "close", &[])?;
    Ok(receiver)
}

/// 提示符写入：优先 `output.write(fn)`，否则直接写 stdout。
fn write_prompt(vm: &mut Vm, output: Option<Value>, text: &str) -> Result<(), VmError> {
    if let Some(o) = output {
        if let Some(or) = o.as_object() {
            if let Ok(w) = vm.get_property(o, "write") {
                if is_callable_value(vm, w) {
                    let arg = Value::Object(vm.alloc_string(text.to_owned()));
                    vm.invoke_callable(w, Value::Object(or), &[arg])?;
                    return Ok(());
                }
            }
        }
    }
    print_direct(text);
    Ok(())
}

/// 经实例自身的 `emit` 方法触发事件（复用 events 分派）。
pub(crate) fn emit_event(
    vm: &mut Vm,
    target: Value,
    event: &str,
    args: &[Value],
) -> Result<(), VmError> {
    let Ok(emit_fn) = vm.get_property(target, "emit") else {
        return Ok(());
    };
    if !is_callable_value(vm, emit_fn) {
        return Ok(());
    }
    let mut full: Vec<Value> = vec![Value::Object(vm.alloc_string(event.to_owned()))];
    full.extend_from_slice(args);
    vm.invoke_callable(emit_fn, target, &full)?;
    Ok(())
}

/// 直接写 stdout 并立即刷新（对齐 Go `fmt.Print` 的即时可见性）。
pub(crate) fn print_direct(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// 阻塞读取一行（含换行符）；无完整行（EOF/残尾）返回 `None`。
///
/// 与 Go `bufio.NewReader(os.Stdin).ReadString('\n')` 行为对齐：每次调用
/// 独立读一块缓冲，残尾内容随 EOF 一并丢弃。
pub(crate) fn read_line_blocking() -> Option<String> {
    let mut leftover = Vec::new();
    read_line_impl(&mut leftover)
}

/// 阻塞读取一行（共享缓冲：行间残留内容保留，供 REPL 循环逐行消费）。
pub(crate) fn read_line_shared() -> Option<String> {
    STDIN_LEFTOVER.with(|leftover| read_line_impl(&mut leftover.borrow_mut()))
}

// stdin 行间残留缓冲（`read_line_shared` 专用，跨调用保留）。
thread_local! {
    // STDIN_LEFTOVER：线程局部（每线程独立的 stdin 消费缓冲）。
    static STDIN_LEFTOVER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// 阻塞读取一行的实现核心（`buf` 为跨调用残留缓冲）。
fn read_line_impl(buf: &mut Vec<u8>) -> Option<String> {
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            return Some(String::from_utf8_lossy(&line).into_owned());
        }
        match lock.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
    }
}

/// 判断值是否可调用（Closure / NativeFn / NativeCtor）。
pub(crate) fn is_callable_value(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
    ))
}

/// 编译期锚定：处理器签名与注册表一致。
/// GC 根快照：Interface 会话的输出流对象。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    RL_STATES.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for st in map.values() {
                if let Some(v) = st.output {
                    out.push(v);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = noop;
        let _: crate::builtins::BuiltinHandler = create_interface;
        let _: crate::builtins::BuiltinHandler = interface_question;
        let _: crate::builtins::BuiltinHandler = interface_set_prompt;
        let _: crate::builtins::BuiltinHandler = interface_get_prompt;
        let _: crate::builtins::BuiltinHandler = interface_prompt;
        let _: crate::builtins::BuiltinHandler = interface_write;
        let _: crate::builtins::BuiltinHandler = interface_get_cursor_pos;
        let _: crate::builtins::BuiltinHandler = interface_pause;
        let _: crate::builtins::BuiltinHandler = interface_resume;
        let _: crate::builtins::BuiltinHandler = interface_close;
    }
}
