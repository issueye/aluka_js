//! `readline/promises` 内置模块（Phase 7）：Promise 版 readline。
//!
//! 语义严格对齐 Node.js 22 LTS 规范：
//! - `createInterface({ input, output })`：Interface 实例的 `question(query)`
//!   返回 `Promise<string>`——先向 `output.write`（或 stdout）写出 query，
//!   再从 `input` 流经 `'data'`/`'end'` 事件消费首行（`'error'` 拒绝）；
//!   无流式 input 时回退阻塞读 stdin（EOF 按流路径语义以空串兑现）；
//! - `Interface` 构造器（`createInterface` 同款实例）；
//! - `Readline` 类：`question`（Promise 化）+ `commit`（返回已兑现 `undefined`
//!   的 Promise）/ `rollback` / `clearLine` / `clearScreenDown` / `cursorTo` /
//!   `moveCursor`（no-op）。
//!
//! 实现说明：`'error'` 路径已随引擎 rejection 语义落地改为真实 Promise 拒绝
//! （原注「暂无 rejection 状态、以兑现近似」已过时）。

use crate::builtins::events::create_emitter_instance;
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::thread::LocalKey;

// 复用 `readline` 模块的阻塞读行 / 直接打印 / 可调用判定 / 事件触发工具。
use crate::builtins::readline::{emit_event, is_callable_value, print_direct, read_line_blocking};

/// `require("readline/promises")` / `require("node:readline/promises")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "readline/promises",
    build,
};

/// 输入流上的挂起读行请求（FIFO 等待队列）。
struct Waiter {
    /// 兑现器句柄（`alloc_promise_resolver` 分配的 resolve 端）
    resolver: ObjectRef,
}

/// 每个输入流一条消费通道：数据监听器只注册一次，块只追加一次；
/// `question` 请求进入等待队列，按 FIFO 兑现（观测语义对齐 Go：顺序
/// question 各取其行）。
#[derive(Default)]
struct InputChannel {
    /// 已累计的输入缓冲
    buf: String,
    /// FIFO 等待队列
    waiters: Vec<Waiter>,
}

/// Interface 实例捕获的输入/输出流（键为实例堆句柄索引）。
struct IfaceEntry {
    /// 输入流（带 `on` 方法的 EventEmitter；可为空）
    input: Option<Value>,
    /// 输出流（带 `write` 方法；可为空）
    output: Option<Value>,
}

// Interface 实例状态表：实例句柄 → 捕获的输入/输出流。
thread_local! {
    // IFACES：线程局部（堆句柄仅本线程 Vm 有效）。
    static IFACES: RefCell<Option<HashMap<u32, IfaceEntry>>> = const { RefCell::new(None) };
}
// 输入流消费通道表：流句柄 → 通道状态。
thread_local! {
    // CHANNELS：线程局部（堆句柄仅本线程 Vm 有效）。
    static CHANNELS: RefCell<Option<HashMap<u32, InputChannel>>> = const { RefCell::new(None) };
}

/// 在状态表上执行闭包（惰性初始化；各表独立访问不嵌套）。
fn with_map<T, F, R>(m: &'static LocalKey<RefCell<Option<HashMap<u32, T>>>>, f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, T>) -> R,
{
    m.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

/// 构建 `readline/promises` 模块对象。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    let create_fn = vm.alloc_native_fn("readline/promises.createInterface");
    set_module_prop(vm, obj, "createInterface", Value::Object(create_fn))?;
    register_handler(
        registry,
        "readline/promises",
        "createInterface",
        create_interface,
    );

    // Interface 类：createInterface 同款实例（Node ≥ 17）。
    let interface_ctor = vm.alloc_native_fn("readline/promises.Interface");
    set_module_prop(vm, obj, "Interface", Value::Object(interface_ctor))?;
    register_handler(registry, "readline/promises", "Interface", create_interface);

    // Readline 类（question/commit/rollback API 面）。
    let readline_ctor_fn = vm.alloc_native_fn("readline/promises.Readline");
    set_module_prop(vm, obj, "Readline", Value::Object(readline_ctor_fn))?;
    register_handler(registry, "readline/promises", "Readline", readline_ctor);

    // 事件消费与实例方法分派（实例 `_builtinNs` = "rlp:iface"）。
    register_handler(registry, "rlp:iface", "onData", stream_on_data);
    register_handler(registry, "rlp:iface", "onEnd", stream_on_end);
    register_handler(registry, "rlp:iface", "onError", stream_on_error);
    register_handler(registry, "rlp:iface", "question", iface_question);
    register_handler(registry, "rlp:iface", "pause", iface_pause);
    register_handler(registry, "rlp:iface", "resume", iface_resume);
    register_handler(registry, "rlp:iface", "close", iface_close);
    register_handler(registry, "rlp:iface", "setPrompt", iface_set_prompt);
    register_handler(registry, "rlp:iface", "commit", proto_commit);
    for method in [
        "rollback",
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "moveCursor",
    ] {
        register_handler(
            registry,
            "rlp:iface",
            &format!("proto.{method}"),
            proto_noop,
        );
    }
    // 事件方法转发（Interface 是 EventEmitter）。
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
            register_handler(registry, "rlp:iface", method, h);
        }
    }
    Ok(obj)
}

/// `createInterface(options)` / `new Interface(options)`。
fn create_interface(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let rl = create_emitter_instance(vm);
    let mut input: Option<Value> = None;
    let mut output: Option<Value> = None;
    if let Some(o) = args.first().copied().as_object {
        if let Ok(v) = vm.get_property(Value::Object(o), "input") {
            if !matches!(v, Value::Undefined) {
                input = Some(v);
            }
        }
        if let Ok(v) = vm.get_property(Value::Object(o), "output") {
            if !matches!(v, Value::Undefined) {
                output = Some(v);
            }
        }
    }
    // 实例命名空间（CALL_METHOD 分派 + 事件方法转发，见 `build`）。
    let ns = Value::Object(vm.alloc_string("rlp:iface".to_owned()));
    set_module_prop(vm, rl, "_builtinNs", ns)?;
    let id = rl.0;
    with_map(&IFACES, |m| {
        m.insert(id, IfaceEntry { input, output });
    });
    for method in [
        "question",
        "pause",
        "resume",
        "close",
        "setPrompt",
        "commit",
        "rollback",
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "moveCursor",
    ] {
        let key = match method {
            "question" => "rlp:iface.question".to_owned(),
            "commit" => "rlp:iface.commit".to_owned(),
            "rollback" | "clearLine" | "clearScreenDown" | "cursorTo" | "moveCursor" => {
                format!("rlp:iface.proto.{method}")
            }
            other => format!("rlp:iface.{other}"),
        };
        let fn_ref = vm.alloc_native_fn(&key);
        set_module_prop(vm, rl, method, Value::Object(fn_ref))?;
    }
    Ok(Value::Object(rl))
}

/// `new Readline(stream)`：流上构造 Promise 化读行器（输入输出共用同一流）。
fn readline_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let inst = create_emitter_instance(vm);
    let stream = args
        .first()
        .copied()
        .filter(|v| matches!(v, Value::Object(_)));
    let id = inst.0;
    with_map(&IFACES, |m| {
        m.insert(
            id,
            IfaceEntry {
                input: stream,
                output: stream,
            },
        );
    });
    // 实例命名空间与 question（同 Interface 路径）。
    let ns = Value::Object(vm.alloc_string("rlp:iface".to_owned()));
    set_module_prop(vm, inst, "_builtinNs", ns)?;
    let question_fn = vm.alloc_native_fn("rlp:iface.question");
    set_module_prop(vm, inst, "question", Value::Object(question_fn))?;
    // 原型方法复制为自有属性（对齐 Go 遍历复制）。
    for method in [
        "commit",
        "rollback",
        "clearLine",
        "clearScreenDown",
        "cursorTo",
        "moveCursor",
    ] {
        let key = if method == "commit" {
            "rlp:iface.commit".to_owned()
        } else {
            format!("rlp:iface.proto.{method}")
        };
        let fn_ref = vm.alloc_native_fn(&key);
        set_module_prop(vm, inst, method, Value::Object(fn_ref))?;
    }
    Ok(Value::Object(inst))
}

/// `rl.question(query)`：Promise 化读一行（Interface 与 Readline 实例共用）。
fn iface_question(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if std::env::var("RL_DEBUG").is_ok() {
        eprintln!(
            "[iface_question debug] args len = {}, args = {:?}",
            args.len(),
            args
        );
    }
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(receiver);
    };
    let found = with_map(&IFACES, |m| m.get(&r.0).map(|e| (e.input, e.output)));
    let (input, output) = found.unwrap_or((None, None));
    promise_read_line(vm, args, input, output)
}

/// Promise 化读行核心：写 query → 流消费或 stdin 回退 → 兑现。
fn promise_read_line(
    vm: &mut Vm,
    args: &[Value],
    input: Option<Value>,
    output: Option<Value>,
) -> Result<Value, VmError> {
    let query = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();

    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);

    // 输出 query 到输出流或 stdout（对齐 Go：output 无 write 时才回退 stdout）。
    let mut wrote = false;
    if let Some(o) = output {
        if let Some(or) = o.as_object {
            if let Ok(w) = vm.get_property(o, "write") {
                if is_callable_value(vm, w) {
                    let arg = Value::Object(vm.alloc_string(query.clone()));
                    vm.invoke_callable(w, Value::Object(or), &[arg])?;
                    wrote = true;
                }
            }
        }
    }
    if !wrote && !query.is_empty() {
        print_direct(&query);
    }

    // 输入流分派：带 `on` 方法的流走事件消费，否则回退阻塞读 stdin。
    let has_on = input
        .filter(|v| matches!(v, Value::Object(_)))
        .is_some_and(|v| matches!(vm.get_property(v, "on"), Ok(f) if is_callable_value(vm, f)));
    if has_on {
        let Some(ir) = input.and_then(|v| v.as_object()) else {
            return Ok(Value::Object(promise));
        };
        let id = ir.0;
        // 每个输入流只注册一次数据监听器（重复注册会导致块重复追加）。
        let existed = with_map(&CHANNELS, |m| {
            let existed = m.contains_key(&id);
            m.entry(id).or_default().waiters.push(Waiter { resolver });
            existed
        });
        if !existed {
            // 对齐 Go：先注册 'end' / 'error'，再注册 'data'（同步结束的流能收到 end）。
            for (event, handler) in [
                ("end", "rlp:iface.onEnd"),
                ("error", "rlp:iface.onError"),
                ("data", "rlp:iface.onData"),
            ] {
                let Ok(on_fn) = vm.get_property(Value::Object(ir), "on") else {
                    continue;
                };
                let listener = Value::Object(vm.alloc_native_fn(handler));
                let ev = Value::Object(vm.alloc_string(event.to_owned()));
                vm.invoke_callable(on_fn, Value::Object(ir), &[ev, listener])?;
            }
        }
        return Ok(Value::Object(promise));
    }

    // 回退：阻塞读 stdin。EOF 以空串兑现（对齐 Go 流路径 'end' 语义）。
    let line = read_line_blocking()
        .map(|raw| raw.trim_end_matches(['\r', '\n']).to_owned())
        .unwrap_or_default();
    let value = Value::Object(vm.alloc_string(line));
    vm.microtask_queue
        .push_back(crate::builtins::Job::Call(Value::Object(resolver), value));
    Ok(Value::Object(promise))
}

/// `'data'` 事件：块追加进通道缓冲，遇换行按 FIFO 兑现最旧等待者。
fn stream_on_data(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    if matches!(args.first(), Some(Value::Null)) {
        return Ok(Value::Undefined);
    }
    let chunk = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let to_settle: Vec<(ObjectRef, String)> = with_map(&CHANNELS, |m| {
        let Some(ch) = m.get_mut(&r.0) else {
            return Vec::new();
        };
        ch.buf.push_str(&chunk);
        let mut done = Vec::new();
        while let Some(idx) = ch.buf.find('\n') {
            let line = ch.buf[..idx].trim_end_matches('\r').to_owned();
            ch.buf.drain(..idx + 1);
            // FIFO：最旧等待者优先（顺序 question 各取其行）。
            if ch.waiters.is_empty() {
                break;
            }
            let w = ch.waiters.remove(0);
            done.push((w.resolver, line));
        }
        done
    });
    settle_all(vm, to_settle)
}

/// `'end'` 事件：以缓冲残文（去尾部 `\r\n`）兑现全部等待者。
fn stream_on_end(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let to_settle: Vec<(ObjectRef, String)> = with_map(&CHANNELS, |m| {
        let Some(ch) = m.get_mut(&r.0) else {
            return Vec::new();
        };
        let line = ch.buf.trim_end_matches(['\r', '\n']).to_owned();
        ch.buf.clear();
        let mut done = Vec::new();
        while let Some(w) = ch.waiters.pop() {
            done.push((w.resolver, line.clone()));
        }
        done
    });
    settle_all(vm, to_settle)
}

/// `'error'` 事件：以错误文本**拒绝**全部等待者（引擎已支持 rejection，
/// 对齐 Go 的 reject(err) 语义）。
fn stream_on_error(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let msg = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "readline error".to_owned());
    let to_reject: Vec<ObjectRef> = with_map(&CHANNELS, |m| {
        let Some(ch) = m.get_mut(&r.0) else {
            return Vec::new();
        };
        let mut done = Vec::new();
        while let Some(w) = ch.waiters.pop() {
            done.push(w.resolver);
        }
        done
    });
    for resolver in to_reject {
        let promise = match vm.heap.get(resolver.0 as usize) {
            Some(HeapObject::PromiseResolver { promise, .. }) => *promise,
            _ => continue,
        };
        let err_val = Value::Object(vm.alloc_string(msg.clone()));
        vm.reject_promise(promise, err_val)?;
    }
    Ok(Value::Undefined)
}

/// 批量兑现等待者：兑现器入微任务队列（对齐 Go 的任务边界——当前同步执行
/// 段完整结束后才恢复 `await`，避免 emit 重入导致乱序）。
fn settle_all(vm: &mut Vm, to_settle: Vec<(ObjectRef, String)>) -> Result<Value, VmError> {
    for (resolver, line) in to_settle {
        let value = Value::Object(vm.alloc_string(line));
        vm.microtask_queue
            .push_back(crate::builtins::Job::Call(Value::Object(resolver), value));
    }
    Ok(Value::Undefined)
}

/// `rl.pause()`：链式返回实例。
fn iface_pause(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `rl.resume()`：链式返回实例。
fn iface_resume(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `rl.close()`：触发 `'close'` 并返回实例。
fn iface_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    emit_event(vm, receiver, "close", &[])?;
    Ok(receiver)
}

/// `rl.setPrompt(...)`：no-op（链式返回实例）。
fn iface_set_prompt(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// `Readline.prototype.commit()`：返回已兑现 `undefined` 的 Promise。
fn proto_commit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let p = vm.alloc_fulfilled_promise(Value::Undefined);
    Ok(Value::Object(p))
}

/// 其余原型方法：no-op 返回 `undefined`。
fn proto_noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// 编译期锚定：处理器签名与注册表一致。
/// GC 根快照：Interface 会话的输入/输出流对象。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    IFACES.with(|g| {
        if let Some(map) = g.borrow().as_ref() {
            for st in map.values() {
                if let Some(v) = st.input {
                    out.push(v);
                }
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
        let _: crate::builtins::BuiltinHandler = create_interface;
        let _: crate::builtins::BuiltinHandler = readline_ctor;
        let _: crate::builtins::BuiltinHandler = iface_question;
        let _: crate::builtins::BuiltinHandler = stream_on_data;
        let _: crate::builtins::BuiltinHandler = stream_on_end;
        let _: crate::builtins::BuiltinHandler = stream_on_error;
        let _: crate::builtins::BuiltinHandler = iface_pause;
        let _: crate::builtins::BuiltinHandler = iface_resume;
        let _: crate::builtins::BuiltinHandler = iface_close;
        let _: crate::builtins::BuiltinHandler = iface_set_prompt;
        let _: crate::builtins::BuiltinHandler = proto_commit;
        let _: crate::builtins::BuiltinHandler = proto_noop;
    }
}
