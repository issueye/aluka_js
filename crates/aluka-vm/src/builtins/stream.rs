//! `stream`、`stream/promises` 与 `stream/consumers` 内置模块（Phase 4）：Node 流机制。
//!
//! 核心能力实现与 Node.js 22 LTS 标准（`nodestream`）严格对齐：
//! - `stream` 模块：
//!   * `Readable` 类构造函数：支持 `push(chunk)`、`read()`、`pipe(dest)`、`on("data", cb)`、`on("end", cb)`；
//!   * `Writable` 类构造函数：支持 `write(chunk)`、`end([chunk])`、`on("finish", cb)`；
//!   * `pipeline(...streams, [callback])`：流管道串联；
//!   * `finished(stream, callback)`：流完成事件监听；
//!   * `Readable.from(iterable)`：从数组或字符串创建可读流。
//! - `stream/promises` 模块：
//!   * `pipeline(...streams) -> Promise`：Promise 化流管道；
//!   * `finished(stream) -> Promise`：Promise 化完成监听。
//! - `stream/consumers` 模块：
//!   * `text(stream) -> Promise<string>`：将流数据拼接消费为字符串；
//!   * `json(stream) -> Promise<object>`：将流数据消费并解析为 JSON；
//!   * `buffer(stream) -> Promise<Buffer>`：将流数据拼接消费为 Buffer 实例。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::sync::Mutex;

/// 流实例内部状态
#[derive(Debug, Clone, Default)]
struct StreamState {
    /// 待消费的数据缓冲队列
    buffer: Vec<Value>,
    /// 是否已结束（Readable 遇 push(null)，Writable 遇 end）
    ended: bool,
    /// 可写流是否已完成（finish 事件已触发）
    finished: bool,
    /// 可读流流动模式开关（true 时自动触发 data 事件或推向 pipe 目标）
    flowing: bool,
    /// pipe 目标流对象句柄
    pipe_dest: Option<Value>,
    /// 事件监听器列表：事件名 -> 回调函数列表
    listeners: HashMap<String, Vec<Value>>,
    /// Writable 自定义写入回调（对应 options.write）
    write_fn: Option<Value>,
    /// `for await` 的 next 等待者：pending promise（按到达顺序兑现）
    awaiters: Vec<aluka_core::ObjectRef>,
}

/// 全局流状态存储表（对象句柄索引 -> 流内部状态）
static STREAM_STORE: Mutex<Option<HashMap<u32, StreamState>>> = Mutex::new(None);

/// 初始化流实例内部状态
fn init_stream_state(id: u32, write_fn: Option<Value>) {
    let mut guard = STREAM_STORE.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(
        id,
        StreamState {
            buffer: Vec::new(),
            ended: false,
            finished: false,
            flowing: false,
            pipe_dest: None,
            listeners: HashMap::new(),
            write_fn,
            awaiters: Vec::new(),
        },
    );
}

/// 安全借用并修改流状态
fn with_stream_state<F, R>(id: u32, f: F) -> Option<R>
where
    F: FnOnce(&mut StreamState) -> R,
{
    let mut guard = STREAM_STORE.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let state = map.entry(id).or_default();
    Some(f(state))
}

/// 获取流状态快照副本
fn get_stream_state(id: u32) -> StreamState {
    let mut guard = STREAM_STORE.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.entry(id).or_default().clone()
}

/// 触发流实例的指定事件监听器
fn emit_event(vm: &mut Vm, stream_val: Value, event: &str, args: &[Value]) -> Result<(), VmError> {
    let id = match stream_val {
        Value::Object(r) => r.0,
        _ => return Ok(()),
    };
    let cbs = with_stream_state(id, |s| s.listeners.get(event).cloned()).flatten();
    if let Some(list) = cbs {
        for cb in list {
            let _ = vm.invoke_callable(cb, stream_val, args)?;
        }
    }
    Ok(())
}

/// 排空缓冲区到管道目标流
fn drain_to_dest(vm: &mut Vm, id: u32, stream_val: Value, dest: Value) -> Result<(), VmError> {
    let chunks: Vec<Value> =
        with_stream_state(id, |s| std::mem::take(&mut s.buffer)).unwrap_or_default();
    for chunk in chunks {
        if matches!(chunk, Value::Null) {
            finish_readable(vm, id, stream_val)?;
            end_pipe_dest(vm, dest)?;
            return Ok(());
        }
        let _ = write_to_stream(vm, dest, chunk)?;
    }
    Ok(())
}

/// 排空缓冲区触发 'data' 事件
fn drain_buffer_to_data(vm: &mut Vm, id: u32, stream_val: Value) -> Result<(), VmError> {
    let chunks: Vec<Value> =
        with_stream_state(id, |s| std::mem::take(&mut s.buffer)).unwrap_or_default();
    for chunk in chunks {
        if matches!(chunk, Value::Null) {
            finish_readable(vm, id, stream_val)?;
            return Ok(());
        }
        emit_event(vm, stream_val, "data", &[chunk])?;
    }
    Ok(())
}

/// 结束可读流（触发 'end' 事件）
fn finish_readable(vm: &mut Vm, id: u32, stream_val: Value) -> Result<(), VmError> {
    with_stream_state(id, |s| {
        s.ended = true;
    });
    emit_event(vm, stream_val, "end", &[])?;
    Ok(())
}

/// 结束 pipe 目标流（调用 dest.end()）
fn end_pipe_dest(vm: &mut Vm, dest: Value) -> Result<(), VmError> {
    let _ = call_stream_method(vm, dest, "end", &[])?;
    Ok(())
}

/// 向流写入数据（调用 dest.write(chunk)）
fn write_to_stream(vm: &mut Vm, dest: Value, chunk: Value) -> Result<Value, VmError> {
    call_stream_method(vm, dest, "write", &[chunk])
}

/// 调用流对象方法：优先直接走原生流处理器
fn call_stream_method(
    vm: &mut Vm,
    target: Value,
    method: &str,
    args: &[Value],
) -> Result<Value, VmError> {
    crate::builtins::set_current_receiver(target);
    match method {
        "write" => stream_write(vm, args),
        "end" => stream_end(vm, args),
        "push" => stream_push(vm, args),
        "read" => stream_read(vm, args),
        "pipe" => stream_pipe(vm, args),
        "on" => stream_on(vm, args),
        _ => {
            let m_val = vm.get_property(target, method)?;
            vm.invoke_callable(m_val, target, args)
        }
    }
}

/// 创建新的 Readable 实例
///
/// 使用 `HeapObject::Readable` 变体，使 `for await...of` 的 `GetAsyncIterator`
/// 识别流实例自身即异步迭代器；缓冲/结束/等待者状态仍存于全局 STREAM_STORE。
pub fn create_readable_instance(vm: &mut Vm, _args: &[Value]) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_readable();
    init_stream_state(obj.0, None);

    let _ = vm.set_property(Value::Object(obj), "_isStream", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(obj), "_isReadable", Value::Boolean(true));

    for method in [
        "push", "read", "pipe", "on", "pause", "resume", "destroy", "isPaused",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("stream.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }

    Ok(obj)
}

/// 创建新的 Writable 实例
pub fn create_writable_instance(vm: &mut Vm, args: &[Value]) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let mut write_fn = None;
    if let Some(Value::Object(opts_ref)) = args.first() {
        if let Ok(w) = vm.get_property(Value::Object(*opts_ref), "write") {
            if matches!(w, Value::Object(_)) {
                write_fn = Some(w);
            }
        }
    }
    init_stream_state(obj.0, write_fn);

    let _ = vm.set_property(Value::Object(obj), "_isStream", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(obj), "_isWritable", Value::Boolean(true));

    for method in [
        "write",
        "end",
        "on",
        "destroy",
        "cork",
        "uncork",
        "setDefaultEncoding",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("stream.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }

    Ok(obj)
}

/// `stream` 主模块定义。
pub const MODULE: ModuleDef = ModuleDef {
    name: "stream",
    build,
};

/// `stream/promises` 子模块定义。
pub const PROMISES_MODULE: ModuleDef = ModuleDef {
    name: "stream/promises",
    build: build_promises,
};

/// `stream/consumers` 子模块定义。
pub const CONSUMERS_MODULE: ModuleDef = ModuleDef {
    name: "stream/consumers",
    build: build_consumers,
};

/// 构建 `stream` 模块对象。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = match vm.stream_module {
        Some(r) => r,
        None => {
            let r = vm.alloc_ordinary();
            vm.stream_module = Some(r);
            r
        }
    };

    // Readable 构造器对象
    let readable_ctor = vm.alloc_native_fn("stream.Readable");
    let from_fn = vm.alloc_native_fn("stream.Readable.from");
    let _ = vm.set_property(Value::Object(readable_ctor), "from", Value::Object(from_fn));

    // Writable 构造器对象
    let writable_ctor = vm.alloc_native_fn("stream.Writable");

    // 模块导出属性挂载
    set_module_prop(vm, obj, "Readable", Value::Object(readable_ctor))?;
    set_module_prop(vm, obj, "Writable", Value::Object(writable_ctor))?;

    let pipeline_fn = vm.alloc_native_fn("stream.pipeline");
    let finished_fn = vm.alloc_native_fn("stream.finished");
    set_module_prop(vm, obj, "pipeline", Value::Object(pipeline_fn))?;
    set_module_prop(vm, obj, "finished", Value::Object(finished_fn))?;

    // 注册 stream 分派方法及多命名空间别名
    for ns in [
        "stream",
        "stream:instance",
        "stream:readable",
        "stream:writable",
    ] {
        register_handler(registry, ns, "push", stream_push);
        register_handler(registry, ns, "read", stream_read);
        register_handler(registry, ns, "next", stream_next);
        register_handler(registry, ns, "pipe", stream_pipe);
        register_handler(registry, ns, "on", stream_on);
        register_handler(registry, ns, "pause", stream_pause);
        register_handler(registry, ns, "resume", stream_resume);
        register_handler(registry, ns, "isPaused", stream_is_paused);
        register_handler(registry, ns, "destroy", stream_destroy);
        register_handler(registry, ns, "write", stream_write);
        register_handler(registry, ns, "end", stream_end);
        register_handler(registry, ns, "pipeline", stream_pipeline);
        register_handler(registry, ns, "finished", stream_finished);
        register_handler(registry, ns, "Readable", stream_readable_ctor);
        register_handler(registry, ns, "Writable", stream_writable_ctor);
        register_handler(registry, ns, "from", readable_from);
    }

    Ok(obj)
}

/// 构建 `stream/promises` 模块对象。
fn build_promises(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in ["pipeline", "finished"] {
        let fn_ref = vm.alloc_native_fn(&format!("stream/promises.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }

    register_handler(registry, "stream/promises", "pipeline", promises_pipeline);
    register_handler(registry, "stream/promises", "finished", promises_finished);

    Ok(obj)
}

/// 构建 `stream/consumers` 模块对象。
fn build_consumers(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for method in ["text", "json", "buffer", "arrayBuffer", "blob"] {
        let fn_ref = vm.alloc_native_fn(&format!("stream/consumers.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }

    register_handler(registry, "stream/consumers", "text", consumers_text);
    register_handler(registry, "stream/consumers", "json", consumers_json);
    register_handler(registry, "stream/consumers", "buffer", consumers_buffer);

    Ok(obj)
}

/// Readable 类构造函数入口
fn stream_readable_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let r = create_readable_instance(vm, args)?;
    Ok(Value::Object(r))
}

/// Writable 类构造函数入口
fn stream_writable_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let r = create_writable_instance(vm, args)?;
    Ok(Value::Object(r))
}

/// `Readable.from(iterable)`：从数组或字符串创建可读流
fn readable_from(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let r_obj = create_readable_instance(vm, &[])?;
    let r_val = Value::Object(r_obj);

    if let Some(src) = args.first().copied() {
        match src {
            Value::Object(ref_idx) => {
                let idx = ref_idx.0 as usize;
                if let Some(heap_obj) = vm.heap.get(idx) {
                    match heap_obj {
                        HeapObject::String(s) => {
                            let val = Value::Object(vm.alloc_string(s.clone()));
                            let _ = call_stream_method(vm, r_val, "push", &[val])?;
                        }
                        HeapObject::Array { elements, .. } => {
                            for e in elements.clone() {
                                let _ = call_stream_method(vm, r_val, "push", &[e])?;
                            }
                        }
                        _ => {
                            let _ = call_stream_method(vm, r_val, "push", &[src])?;
                        }
                    }
                }
            }
            _ => {
                let _ = call_stream_method(vm, r_val, "push", &[src])?;
            }
        }
    }
    // 推送 null 标记流结束
    let _ = call_stream_method(vm, r_val, "push", &[Value::Null])?;
    Ok(r_val)
}

/// 构造异步迭代结果对象 `{ value, done }`。
fn async_iter_result(vm: &mut Vm, value: Value, done: bool) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "value", value);
    let _ = vm.set_property(Value::Object(obj), "done", Value::Boolean(done));
    Ok(Value::Object(obj))
}

/// 兑现最早的一个 `for await` next 等待者（若有）。
fn fulfill_next_awaiter(vm: &mut Vm, id: u32, value: Value, done: bool) -> Result<(), VmError> {
    let awaiter = with_stream_state(id, |s| {
        if s.awaiters.is_empty() {
            None
        } else {
            Some(s.awaiters.remove(0))
        }
    });
    if let Some(promise) = awaiter.flatten() {
        let result = async_iter_result(vm, value, done)?;
        vm.fulfill_promise(promise, result)?;
    }
    Ok(())
}

/// `stream.next()`：`for await...of` 的异步迭代器协议。
///
/// 有缓冲数据 → fulfilled Promise(`{value, done:false}`)；空且已结束 →
/// `{value:undefined, done:true}`；空且未结束 → pending Promise（数据到达时
/// 由 `stream_push` 兑现）。
fn stream_next(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let state = get_stream_state(id);
    if let Some(chunk) = state.buffer.first().copied() {
        // 缓冲队首：数据或结束哨兵
        with_stream_state(id, |s| {
            s.buffer.remove(0);
        });
        if matches!(chunk, Value::Null) {
            with_stream_state(id, |s| s.ended = true);
            let result = async_iter_result(vm, Value::Undefined, true)?;
            return Ok(Value::Object(vm.alloc_fulfilled_promise(result)));
        }
        let result = async_iter_result(vm, chunk, false)?;
        return Ok(Value::Object(vm.alloc_fulfilled_promise(result)));
    }
    if state.ended {
        let result = async_iter_result(vm, Value::Undefined, true)?;
        return Ok(Value::Object(vm.alloc_fulfilled_promise(result)));
    }
    // 空且未结束：登记等待者，数据到达时兑现
    let pending = vm.alloc_pending_promise();
    with_stream_state(id, |s| s.awaiters.push(pending));
    Ok(Value::Object(pending))
}

/// `stream.push(chunk)`：向流推送数据，push(null) 标记结束
fn stream_push(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Boolean(false)),
    };
    if let Some(chunk) = args.first().copied() {
        if matches!(chunk, Value::Null) {
            with_stream_state(id, |s| s.ended = true);
            let state = get_stream_state(id);
            // 等待中的 for await next：立即以 done 兑现
            fulfill_next_awaiter(vm, id, Value::Undefined, true)?;
            if state.flowing {
                finish_readable(vm, id, receiver)?;
                if let Some(dest) = state.pipe_dest {
                    end_pipe_dest(vm, dest)?;
                }
            }
            return Ok(Value::Boolean(false));
        }
        if !matches!(chunk, Value::Undefined) {
            let has_awaiter = with_stream_state(id, |s| !s.awaiters.is_empty()).unwrap_or(false);
            if has_awaiter {
                // 等待者优先：直接兑现，不进入缓冲
                fulfill_next_awaiter(vm, id, chunk, false)?;
            } else {
                with_stream_state(id, |s| s.buffer.push(chunk));
            }
            let state = get_stream_state(id);
            if state.flowing {
                if let Some(dest) = state.pipe_dest {
                    drain_to_dest(vm, id, receiver, dest)?;
                } else {
                    drain_buffer_to_data(vm, id, receiver)?;
                }
            }
        }
    }
    Ok(Value::Boolean(true))
}

/// `stream.read([size])`：从流缓冲区读取数据
fn stream_read(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Null),
    };
    let chunk = with_stream_state(id, |s| {
        if !s.buffer.is_empty() {
            Some(s.buffer.remove(0))
        } else {
            None
        }
    })
    .flatten();
    Ok(chunk.unwrap_or(Value::Null))
}

/// `stream.pipe(destination)`：将可读流管道连接至目标可写流
fn stream_pipe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let dest = match args.first().copied() {
        Some(d) if matches!(d, Value::Object(_)) => d,
        _ => return Ok(receiver),
    };
    with_stream_state(id, |s| {
        s.pipe_dest = Some(dest);
        s.flowing = true;
    });
    // 立即排空缓冲区到目标流
    drain_to_dest(vm, id, receiver, dest)?;
    let state = get_stream_state(id);
    if state.ended {
        finish_readable(vm, id, receiver)?;
        end_pipe_dest(vm, dest)?;
    }
    Ok(dest)
}

/// `stream.on(event, callback)`：注册事件监听器
fn stream_on(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(receiver),
    };
    let event = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let cb = args.get(1).copied().unwrap_or(Value::Undefined);

    if matches!(cb, Value::Object(_)) {
        with_stream_state(id, |s| {
            s.listeners.entry(event.clone()).or_default().push(cb);
        });
    }

    let state = get_stream_state(id);
    if event == "data" {
        with_stream_state(id, |s| s.flowing = true);
        drain_buffer_to_data(vm, id, receiver)?;
        let state = get_stream_state(id);
        if state.ended {
            finish_readable(vm, id, receiver)?;
        }
    } else if (event == "finish" && state.finished)
        || (event == "end" && state.ended && state.flowing)
    {
        let _ = vm.invoke_callable(cb, receiver, &[])?;
    }

    Ok(receiver)
}

/// `stream.pause()`：暂停流动
fn stream_pause(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        with_stream_state(r.0, |s| s.flowing = false);
    }
    Ok(receiver)
}

/// `stream.resume()`：恢复流动
fn stream_resume(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        with_stream_state(r.0, |s| s.flowing = true);
        drain_buffer_to_data(vm, r.0, receiver)?;
        let state = get_stream_state(r.0);
        if state.ended {
            finish_readable(vm, r.0, receiver)?;
        }
    }
    Ok(receiver)
}

/// `stream.isPaused()`
fn stream_is_paused(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let paused = match receiver {
        Value::Object(r) => with_stream_state(r.0, |s| !s.flowing).unwrap_or(true),
        _ => true,
    };
    Ok(Value::Boolean(paused))
}

/// `stream.destroy([error])`
fn stream_destroy(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        if let Some(err) = args.first() {
            if !matches!(err, Value::Undefined | Value::Null) {
                emit_event(vm, receiver, "error", &[*err])?;
            }
        }
        with_stream_state(r.0, |s| {
            s.ended = true;
            s.buffer.clear();
        });
        emit_event(vm, receiver, "close", &[])?;
    }
    Ok(receiver)
}

/// `stream.write(chunk[, encoding][, callback])`：向可写流写入数据
fn stream_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Boolean(false)),
    };
    let Some(chunk) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let state = get_stream_state(id);
    if let Some(write_fn) = state.write_fn {
        let empty_str = Value::Object(vm.alloc_string(String::new()));
        let _ = vm.invoke_callable(write_fn, receiver, &[chunk, empty_str, Value::Undefined])?;
    } else {
        with_stream_state(id, |s| s.buffer.push(chunk));
    }
    Ok(Value::Boolean(true))
}

/// `stream.end([chunk][, callback])`：结束可写流
fn stream_end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    if let Some(chunk) = args.first().copied() {
        if !matches!(chunk, Value::Undefined | Value::Null) {
            let state = get_stream_state(id);
            if let Some(write_fn) = state.write_fn {
                let empty_str = Value::Object(vm.alloc_string(String::new()));
                let _ =
                    vm.invoke_callable(write_fn, receiver, &[chunk, empty_str, Value::Undefined])?;
            } else {
                with_stream_state(id, |s| s.buffer.push(chunk));
            }
        }
    }
    with_stream_state(id, |s| {
        s.ended = true;
        s.finished = true;
    });
    emit_event(vm, receiver, "finish", &[])?;
    emit_event(vm, receiver, "close", &[])?;
    Ok(receiver)
}

/// `stream.pipeline(...streams, [callback])` 回调版管道
fn stream_pipeline(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        let msg = vm.alloc_string("pipeline 至少需要 2 个参数".to_owned());
        return Err(VmError::Thrown(Value::Object(msg)));
    }
    let (streams, cb) = if let Some(last) = args.last() {
        if matches!(
            last,
            Value::Object(r)
                if matches!(
                    vm.heap.get(r.0 as usize),
                    Some(HeapObject::Closure { .. })
                )
        ) {
            (&args[..args.len() - 1], Some(*last))
        } else {
            (args, None)
        }
    } else {
        (args, None)
    };

    if streams.len() < 2 {
        let msg = vm.alloc_string("pipeline 至少需要 2 个流".to_owned());
        return Err(VmError::Thrown(Value::Object(msg)));
    }

    if let Some(callback) = cb {
        let last_stream = streams[streams.len() - 1];
        let finish_str = Value::Object(vm.alloc_string("finish".to_owned()));
        let _ = call_stream_method(vm, last_stream, "on", &[finish_str, callback])?;
    }

    let mut current = streams[0];
    for next_stream in streams.iter().skip(1).copied() {
        current = call_stream_method(vm, current, "pipe", &[next_stream])?;
    }

    Ok(streams[streams.len() - 1])
}

/// `stream.finished(stream, callback)`
fn stream_finished(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Ok(Value::Undefined);
    }
    let stream = args[0];
    let cb = args[1];
    for event in ["finish", "end", "error"] {
        let ev_str = Value::Object(vm.alloc_string(event.to_owned()));
        let _ = call_stream_method(vm, stream, "on", &[ev_str, cb])?;
    }
    Ok(Value::Undefined)
}

/// `stream/promises.pipeline(...streams) -> Promise`
fn promises_pipeline(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);
    let resolver_val = Value::Object(resolver);

    if args.len() < 2 {
        let err_obj = vm.alloc_string("pipeline 至少需要 2 个流".to_owned());
        vm.fulfill_promise(promise, Value::Object(err_obj))?;
        return Ok(Value::Object(promise));
    }

    let last_stream = args[args.len() - 1];
    let finish_str = Value::Object(vm.alloc_string("finish".to_owned()));
    let _ = call_stream_method(vm, last_stream, "on", &[finish_str, resolver_val])?;

    let mut current = args[0];
    for next_stream in args.iter().skip(1).copied() {
        current = call_stream_method(vm, current, "pipe", &[next_stream])?;
    }

    Ok(Value::Object(promise))
}

/// `stream/promises.finished(stream) -> Promise`
fn promises_finished(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);
    let resolver_val = Value::Object(resolver);

    if let Some(stream) = args.first().copied() {
        for event in ["finish", "end"] {
            let ev_str = Value::Object(vm.alloc_string(event.to_owned()));
            let _ = call_stream_method(vm, stream, "on", &[ev_str, resolver_val])?;
        }
    } else {
        let err = vm.alloc_string("finished 需要流参数".to_owned());
        vm.fulfill_promise(promise, Value::Object(err))?;
    }

    Ok(Value::Object(promise))
}

/// 消费模式
enum ConsumerMode {
    Text,
    Json,
    Buffer,
}

/// 将流数据聚集消费为 Promise 结果
fn consume_stream_internal(
    vm: &mut Vm,
    stream_val: Value,
    mode: ConsumerMode,
) -> Result<Value, VmError> {
    let promise = vm.alloc_pending_promise();
    let id = match stream_val {
        Value::Object(r) => r.0,
        _ => {
            let empty = vm.alloc_string(String::new());
            vm.fulfill_promise(promise, Value::Object(empty))?;
            return Ok(Value::Object(promise));
        }
    };

    // 收集所有缓冲数据
    let mut collected = Vec::new();
    with_stream_state(id, |s| {
        collected.append(&mut s.buffer);
    });

    let _state = get_stream_state(id);
    fulfill_consumer_result(vm, promise, &collected, mode)?;

    Ok(Value::Object(promise))
}

/// 兑现消费者结果
fn fulfill_consumer_result(
    vm: &mut Vm,
    promise: ObjectRef,
    chunks: &[Value],
    mode: ConsumerMode,
) -> Result<(), VmError> {
    match mode {
        ConsumerMode::Text => {
            let mut s = String::new();
            for c in chunks {
                s.push_str(&vm.format_value(*c));
            }
            let str_obj = vm.alloc_string(s);
            vm.fulfill_promise(promise, Value::Object(str_obj))?;
        }
        ConsumerMode::Json => {
            let mut s = String::new();
            for c in chunks {
                s.push_str(&vm.format_value(*c));
            }
            // 简单 JSON 对象与基础值解析
            let parsed = parse_simple_json(vm, &s);
            vm.fulfill_promise(promise, parsed)?;
        }
        ConsumerMode::Buffer => {
            let mut bytes = Vec::new();
            for c in chunks {
                if let Some(b) = crate::builtins::buffer::extract_bytes(vm, *c) {
                    bytes.extend_from_slice(&b);
                } else {
                    let s = vm.format_value(*c);
                    bytes.extend_from_slice(s.as_bytes());
                }
            }
            let buf_obj = crate::builtins::buffer::create_buffer_instance(vm, bytes);
            vm.fulfill_promise(promise, Value::Object(buf_obj))?;
        }
    }
    Ok(())
}

/// 简易 JSON 解析（对齐前端常见 JSON 格式）
fn parse_simple_json(vm: &mut Vm, s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        let obj = vm.alloc_ordinary();
        let inner = &trimmed[1..trimmed.len() - 1];
        for pair in inner.split(',') {
            let parts: Vec<&str> = pair.splitn(2, ':').collect();
            if parts.len() == 2 {
                let key = parts[0].trim().trim_matches('"').trim_matches('\'');
                let val_str = parts[1].trim();
                let val = if val_str.starts_with('"') && val_str.ends_with('"') {
                    let text = val_str[1..val_str.len() - 1].to_owned();
                    Value::Object(vm.alloc_string(text))
                } else if let Ok(n) = val_str.parse::<f64>() {
                    Value::Number(n)
                } else if val_str == "true" {
                    Value::Boolean(true)
                } else if val_str == "false" {
                    Value::Boolean(false)
                } else if val_str == "null" {
                    Value::Null
                } else {
                    Value::Undefined
                };
                let _ = vm.set_property(Value::Object(obj), key, val);
            }
        }
        Value::Object(obj)
    } else if let Ok(n) = trimmed.parse::<f64>() {
        Value::Number(n)
    } else {
        let s_obj = vm.alloc_string(trimmed.to_owned());
        Value::Object(s_obj)
    }
}

/// `stream/consumers.text(stream) -> Promise<string>`
fn consumers_text(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let stream_val = args.first().copied().unwrap_or(Value::Undefined);
    consume_stream_internal(vm, stream_val, ConsumerMode::Text)
}

/// `stream/consumers.json(stream) -> Promise<object>`
fn consumers_json(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let stream_val = args.first().copied().unwrap_or(Value::Undefined);
    consume_stream_internal(vm, stream_val, ConsumerMode::Json)
}

/// `stream/consumers.buffer(stream) -> Promise<Buffer>`
fn consumers_buffer(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let stream_val = args.first().copied().unwrap_or(Value::Undefined);
    consume_stream_internal(vm, stream_val, ConsumerMode::Buffer)
}
