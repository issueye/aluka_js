//! `stream/web` 内置模块（Phase 4）：Web Streams 构造器表面 + `ReadableStreamTee`。
//!
//! Node.js 22 LTS 规范把全局
//! `ReadableStream` / `WritableStream` / `TransformStream` 转发为模块导出并提供
//! `ReadableStreamTee`；Rust 引擎尚无这些全局，故在本模块内直接定义三个构造器
//! 的最小表面——构造、属性存在性（`locked` / `getReader` / `tee` / `getWriter`
//! / `readable` / `writable` 等）与基本身份——语义实测对齐 Go 全局实现
//! （`runtime/globals/gstream/streams.go`）：
//! - `new ReadableStream({ start })`：同步调用 `start(controller)`，controller
//!   提供 `enqueue` / `close` / `error`；
//! - `getReader()` 把 `locked` 置真并返回带 `read` / `cancel` / `releaseLock`
//!   的 reader；
//! - `tee()` 返回 `[stream, stream]`（同一对象身份，对齐 Go 简化实现）；
//! - `ReadableStreamTee(stream)`：优先调用 `stream.tee()`，否则返回
//!   `[stream, stream]`，无参返回 undefined，非对象实参抛错。

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, register_handler, set_module_prop,
};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("stream/web")` / `require("node:stream/web")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "stream/web",
    build,
};

/// ReadableStream 实例方法命名空间（挂 `_builtinNs` 供通用分派）。
const RS_NS: &str = "stream/web:rs";
/// reader 实例方法命名空间。
const READER_NS: &str = "stream/web:reader";
/// WritableStream 实例方法命名空间。
const WS_NS: &str = "stream/web:ws";
/// writer 实例方法命名空间。
const WRITER_NS: &str = "stream/web:writer";
/// controller（start 回调实参）方法命名空间。
const CTL_NS: &str = "stream/web:ctl";

/// ReadableStream 内部状态：chunk 队列 + 关闭标记 + 所属对象句柄
/// （以堆对象 id 为键，对齐 buffer 模块的 BUFFER_STORE 模式）。
struct RsState {
    queue: VecDeque<Value>,
    closed: bool,
    stream: ObjectRef,
    /// M4 互通桥（`Readable.fromWeb`）：数据/关闭同步转发到的 Node 可读流 id。
    bridge: Option<u32>,
}

// 全部 ReadableStream 的内部状态表（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static RS_STATES: RefCell<Option<HashMap<u32, RsState>>> = const { RefCell::new(None) };
}

/// WritableStream 内部状态：underlyingSink 回调 + Node 桥（toWeb）。
struct WsState {
    /// `underlyingSink.write(chunk)`（JS 回调，GC 根登记见 store_roots）
    sink_write: Option<Value>,
    /// `underlyingSink.close()`
    sink_close: Option<Value>,
    /// `Writable.toWeb` 桥：writer 写入转发到的 Node 可写流。
    node_sink: Option<ObjectRef>,
}

// 全部 WritableStream 的内部状态表（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static WS_STATES: RefCell<Option<HashMap<u32, WsState>>> = const { RefCell::new(None) };
}

fn with_state<R>(id: u32, f: impl FnOnce(&mut RsState) -> R) -> Option<R> {
    RS_STATES.with(|g| g.borrow_mut().as_mut()?.get_mut(&id).map(f))
}

fn insert_state(id: u32, state: RsState) {
    RS_STATES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, state);
    });
}

/// M4 GC 根登记：两张状态表持有的全部堆值（chunk 队列 / sink 回调 / Node 桥）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    RS_STATES.with(|g| {
        let binding = g.borrow();
        if let Some(map) = binding.as_ref() {
            for s in map.values() {
                for v in &s.queue {
                    out.push(*v);
                }
                out.push(Value::Object(s.stream));
            }
        }
    });
    WS_STATES.with(|g| {
        let binding = g.borrow();
        if let Some(map) = binding.as_ref() {
            for s in map.values() {
                if let Some(w) = s.sink_write {
                    out.push(w);
                }
                if let Some(c) = s.sink_close {
                    out.push(c);
                }
                if let Some(n) = s.node_sink {
                    out.push(Value::Object(n));
                }
            }
        }
    });
}

/// M4 互通：登记 fromWeb 桥并补交既有队列（`start` 阶段先于挂桥入队的
/// chunk 与已关闭哨兵不丢——对齐 Node `Readable.fromWeb` 的可见语义）。
pub fn attach_node_bridge_and_drain(vm: &mut Vm, web_id: u32, node_id: u32) -> Result<(), VmError> {
    let mut pending: Vec<Value> = Vec::new();
    {
        RS_STATES.with(|g| {
            if let Some(state) = g.borrow_mut().as_mut().and_then(|m| m.get_mut(&web_id)) {
                state.bridge = Some(node_id);
                pending = state.queue.drain(..).collect();
                if state.closed {
                    pending.push(Value::Null);
                }
            }
        });
    }
    for chunk in pending {
        crate::builtins::stream::node_bridge_push(vm, node_id, chunk)?;
    }
    Ok(())
}

/// M4 互通：Node 侧 push 转发入口——chunk 入 web 队列。
pub fn web_bridge_enqueue(web_id: u32, chunk: Value) {
    RS_STATES.with(|g| {
        if let Some(state) = g.borrow_mut().as_mut().and_then(|m| m.get_mut(&web_id)) {
            state.queue.push_back(chunk);
        }
    });
}

/// M4 互通：Node 侧 push(null) 转发入口——关闭 web 流。
pub fn web_bridge_close(web_id: u32) {
    RS_STATES.with(|g| {
        if let Some(state) = g.borrow_mut().as_mut().and_then(|m| m.get_mut(&web_id)) {
            state.closed = true;
        }
    });
}

/// 查询 WritableStream 状态（writer / toWeb 桥用）。
fn with_ws_state<R>(id: u32, f: impl FnOnce(&mut WsState) -> R) -> Option<R> {
    WS_STATES.with(|g| g.borrow_mut().as_mut()?.get_mut(&id).map(f))
}

/// 读取接收者上的 `_wsId`（WritableStream / writer 共用关联键）。
fn receiver_ws_id(vm: &mut Vm) -> Option<u32> {
    let this = crate::builtins::current_receiver();
    match vm.get_property(this, "_wsId") {
        Ok(Value::Number(n)) if n >= 0.0 => Some(n as u32),
        _ => None,
    }
}

/// M4 互通：为 `Writable.toWeb` 创建真实 WritableStream 实例（无 underlyingSink）。
pub fn create_web_writable(vm: &mut Vm) -> Result<ObjectRef, VmError> {
    writable_stream_ctor(vm, &[]).map(|v| match v {
        Value::Object(r) => r,
        _ => unreachable!("writable_stream_ctor 恒返回对象"),
    })
}

/// M4 互通：登记 toWeb 桥（web 流 id → Node 可写流）。
pub fn attach_node_sink(web_id: u32, node: ObjectRef) {
    WS_STATES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .entry(web_id)
            .or_insert(WsState {
                sink_write: None,
                sink_close: None,
                node_sink: None,
            })
            .node_sink = Some(node);
    });
}

/// M4 互通：为 `Writable.fromWeb` 回写 web 流的 underlyingSink 回调
/// （Node 可写流 → web sink 的转发目标）。
pub fn set_web_sink_fns(web_id: u32, sink_write: Option<Value>, sink_close: Option<Value>) {
    WS_STATES.with(|g| {
        let mut binding = g.borrow_mut();
        binding
            .get_or_insert_with(HashMap::new)
            .entry(web_id)
            .or_insert(WsState {
                sink_write: None,
                sink_close: None,
                node_sink: None,
            });
        if let Some(state) = binding.as_mut().and_then(|m| m.get_mut(&web_id)) {
            if state.sink_write.is_none() {
                state.sink_write = sink_write;
            }
            if state.sink_close.is_none() {
                state.sink_close = sink_close;
            }
        }
    });
}

// --- 通用辅助 ---------------------------------------------------------------

/// 抛出 JS 异常（字符串消息）。
fn thrown(vm: &mut Vm, message: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_string(message.to_owned())))
}

/// 判断值是否为可调用函数（JS 闭包或原生函数）。
fn is_function(vm: &Vm, val: Value) -> bool {
    matches!(val, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(crate::heap::HeapObject::Closure { .. } | crate::heap::HeapObject::NativeFn { .. })
    ))
}

/// 分派命名空间写入：给实例挂 `_builtinNs` 命名串。
fn set_ns(vm: &mut Vm, obj: Value, ns: &str) {
    let ns_ref = Value::Object(vm.alloc_string(ns.to_owned()));
    let _ = vm.set_property(obj, "_builtinNs", ns_ref);
}

/// 在实例上挂一个原生方法属性（名称即分派键）。
fn set_method(vm: &mut Vm, obj: Value, ns: &str, method: &str) {
    let fn_ref = vm.alloc_native_fn(&format!("{ns}.{method}"));
    let _ = vm.set_property(obj, method, Value::Object(fn_ref));
}

/// 读取当前接收者（this）上记录的所属流 id。
fn receiver_stream_id(vm: &mut Vm) -> Option<u32> {
    let this = crate::builtins::current_receiver();
    match vm.get_property(this, "_streamId") {
        Ok(Value::Number(n)) if n >= 0.0 => Some(n as u32),
        _ => None,
    }
}

/// 构造已兑现 Promise（fs_promises 的 wrap_promise 模式：微任务投递兑现）。
fn resolved_promise(vm: &mut Vm, val: Value) -> Result<Value, VmError> {
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_promise_resolver(promise, true);
    crate::builtins::timers::set_resolver_val(resolver.0, val);
    vm.microtask_queue
        .push_back(crate::builtins::Job::Call(Value::Object(resolver), val));
    Ok(Value::Object(promise))
}

/// 构造 `{ value, done }` 读取结果对象。
fn read_result(vm: &mut Vm, value: Value, done: bool) -> Value {
    let obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "value", value);
    let _ = vm.set_property(Value::Object(obj), "done", Value::Boolean(done));
    Value::Object(obj)
}

// --- 构造器 -----------------------------------------------------------------

/// `new ReadableStream([underlyingSource])`：登记内部状态、挂最小表面并同步
/// 调用 `start(controller)`（对齐 Go gstream.NewReadableStream）。
pub(crate) fn readable_stream_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let stream = vm.alloc_ordinary();
    insert_state(
        stream.0,
        RsState {
            queue: VecDeque::new(),
            closed: false,
            stream,
            bridge: None,
        },
    );
    let stream_val = Value::Object(stream);
    set_ns(vm, stream_val, RS_NS);
    // 流实例自身也记 `_streamId`：公开的 `enqueue` / `close` 方法以流为接收者。
    let _ = vm.set_property(stream_val, "_streamId", Value::Number(stream.0 as f64));
    let _ = vm.set_property(stream_val, "locked", Value::Boolean(false));
    for method in ["getReader", "tee", "cancel", "enqueue", "close", "pipeTo"] {
        set_method(vm, stream_val, RS_NS, method);
    }

    // controller：enqueue / close / error，经 `_streamId` 关联本流。
    let controller = vm.alloc_ordinary();
    let controller_val = Value::Object(controller);
    set_ns(vm, controller_val, CTL_NS);
    let _ = vm.set_property(controller_val, "_streamId", Value::Number(stream.0 as f64));
    for method in ["enqueue", "close", "error"] {
        set_method(vm, controller_val, CTL_NS, method);
    }

    // 同步调用 start 回调（Go 侧同样同步）。
    if let Some(&source) = args.first() {
        if let Ok(start) = vm.get_property(source, "start") {
            if is_function(vm, start) {
                vm.invoke_callable(start, Value::Undefined, &[controller_val])?;
            }
        }
    }
    Ok(stream_val)
}

/// `new WritableStream([underlyingSink])`：登记 underlyingSink 回调与表面
/// `getWriter` / `write` / `close`（M4 互通：sink 回调供 fromWeb 转发）。
pub(crate) fn writable_stream_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let stream = vm.alloc_ordinary();
    let mut sink_write = None;
    let mut sink_close = None;
    if let Some(sink_ref) = args.first().copied().as_object {
        let sink = Value::Object(sink_ref);
        if let Ok(w) = vm.get_property(sink, "write") {
            if is_function(vm, w) {
                sink_write = Some(w);
            }
        }
        if let Ok(c) = vm.get_property(sink, "close") {
            if is_function(vm, c) {
                sink_close = Some(c);
            }
        }
    }
    WS_STATES.with(|g| {
        g.borrow_mut().get_or_insert_with(HashMap::new).insert(
            stream.0,
            WsState {
                sink_write,
                sink_close,
                node_sink: None,
            },
        );
    });

    let stream_val = Value::Object(stream);
    set_ns(vm, stream_val, WS_NS);
    let _ = vm.set_property(stream_val, "_wsId", Value::Number(stream.0 as f64));
    for method in ["getWriter", "write", "close"] {
        set_method(vm, stream_val, WS_NS, method);
    }
    Ok(stream_val)
}

/// `new TransformStream([transformer])`：`readable` / `writable` 两端为真实
/// 构造器实例（属性存在性与身份探针可对拍）。
pub(crate) fn transform_stream_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let ts = vm.alloc_ordinary();
    let readable = readable_stream_ctor(vm, &[])?;
    let writable = writable_stream_ctor(vm, &[])?;
    let ts_val = Value::Object(ts);
    let _ = vm.set_property(ts_val, "readable", readable);
    let _ = vm.set_property(ts_val, "writable", writable);
    Ok(ts_val)
}

// --- ReadableStream 实例方法 -------------------------------------------------

/// `rs.getReader()`：置 `locked` 并返回 reader（`read` / `cancel` / `releaseLock`）。
fn rs_get_reader(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    let _ = vm.set_property(this, "locked", Value::Boolean(true));
    let reader = vm.alloc_ordinary();
    let reader_val = Value::Object(reader);
    set_ns(vm, reader_val, READER_NS);
    let _ = vm.set_property(
        reader_val,
        "_streamId",
        Value::Number(this_id(&this) as f64),
    );
    for method in ["read", "cancel", "releaseLock"] {
        set_method(vm, reader_val, READER_NS, method);
    }
    Ok(reader_val)
}

/// 读取接收者对象的堆 id（`_streamId` 写入时已保证为流关联对象）。
fn this_id(this: &Value) -> u32 {
    match this {
        Value::Object(r) => r.0,
        _ => 0,
    }
}

/// `rs.tee()`：返回 `[stream, stream]`（同一对象身份，对齐 Go 简化实现）。
fn rs_tee(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    Ok(Value::Object(vm.alloc_array(vec![this, this])))
}

/// `rs.cancel()`：关闭流并返回已兑现 Promise。
fn rs_cancel(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_stream_id(vm) {
        with_state(id, |s| s.closed = true);
    }
    resolved_promise(vm, Value::Undefined)
}

/// `rs.enqueue(chunk)` / `controller.enqueue(chunk)`：推入内部队列；
/// 存在 fromWeb 桥时同步转发到 Node 可读流（live 互通）。
fn rs_enqueue(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_stream_id(vm) {
        let chunk = args.first().copied().unwrap_or(Value::Undefined);
        if let Some((Some(node_id), false)) = with_state(id, |s| (s.bridge, s.closed)) {
            crate::builtins::stream::node_bridge_push(vm, node_id, chunk)?;
        }
        with_state(id, |s| s.queue.push_back(chunk));
    }
    Ok(Value::Undefined)
}

/// `rs.close()` / `controller.close()`：关闭流；fromWeb 桥同步推 Null 哨兵。
fn rs_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_stream_id(vm) {
        if let Some((Some(node_id), false)) = with_state(id, |s| (s.bridge, s.closed)) {
            crate::builtins::stream::node_bridge_push(vm, node_id, Value::Null)?;
        }
        with_state(id, |s| s.closed = true);
    }
    Ok(Value::Undefined)
}

/// `controller.error()`：标记错误态（简化为关闭，对齐 Go 的 error 行为面）；
/// fromWeb 桥同步推 Null 哨兵终结 Node 侧消费。
fn ctl_error(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_stream_id(vm) {
        if let Some((Some(node_id), false)) = with_state(id, |s| (s.bridge, s.closed)) {
            crate::builtins::stream::node_bridge_push(vm, node_id, Value::Null)?;
        }
        with_state(id, |s| s.closed = true);
    }
    Ok(Value::Undefined)
}

/// `rs.pipeTo(dest)`：最小表面仅保证属性存在性（Go 侧为异步导流）。
fn rs_pipe_to(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

// --- reader 实例方法 ---------------------------------------------------------

/// `reader.read()`：取队首 chunk；空队列或已关闭返回 `{ value, done: true }`。
fn reader_read(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_stream_id(vm).unwrap_or(0);
    let next = with_state(id, |s| {
        s.queue
            .pop_front()
            .map(|v| (v, false))
            .unwrap_or((Value::Undefined, true))
    });
    let (value, done) = next.unwrap_or((Value::Undefined, true));
    let result = read_result(vm, value, done);
    resolved_promise(vm, result)
}

/// `reader.cancel()`：关闭关联流并返回已兑现 Promise。
fn reader_cancel(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if let Some(id) = receiver_stream_id(vm) {
        with_state(id, |s| s.closed = true);
    }
    resolved_promise(vm, Value::Undefined)
}

/// `reader.releaseLock()`：把关联流的 `locked` 置回 false。
fn reader_release_lock(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_stream_id(vm).unwrap_or(0);
    let stream = with_state(id, |s| s.stream);
    if let Some(stream) = stream {
        let _ = vm.set_property(Value::Object(stream), "locked", Value::Boolean(false));
    }
    Ok(Value::Undefined)
}

// --- WritableStream / writer 实例方法 ----------------------------------------

/// `ws.getWriter()`：返回 writer（`write` / `close`，方法均返回已兑现 Promise）；
/// writer 记 `_wsId` 关联所属流（M4 互通转发键）。
fn ws_get_writer(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = crate::builtins::current_receiver();
    let writer = vm.alloc_ordinary();
    let writer_val = Value::Object(writer);
    set_ns(vm, writer_val, WRITER_NS);
    let ws_id_prop = vm.get_property(this, "_wsId").unwrap_or(Value::Undefined);
    let _ = vm.set_property(writer_val, "_wsId", ws_id_prop);
    for method in ["write", "close"] {
        set_method(vm, writer_val, WRITER_NS, method);
    }
    Ok(writer_val)
}

/// WritableStream 写入统一路径：优先 Node 桥（toWeb），否则 underlyingSink.write；
/// 两者皆无则静默吞下（对齐 Go 最小实现）。恒返回已兑现 Promise。
fn ws_write_via(vm: &mut Vm, ws_id: u32, chunk: Value) -> Result<Value, VmError> {
    let (node_sink, sink_write) =
        with_ws_state(ws_id, |s| (s.node_sink, s.sink_write)).unwrap_or((None, None));
    if let Some(node) = node_sink {
        let node_val = Value::Object(node);
        let _ = crate::builtins::stream::call_stream_method(vm, node_val, "write", &[chunk])?;
    } else if let Some(w) = sink_write {
        vm.invoke_callable(w, Value::Undefined, &[chunk])?;
    }
    resolved_promise(vm, Value::Undefined)
}

/// `ws.write(chunk)`：直调写入路径（`_wsId` 在流实例上）。
fn ws_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    let id = receiver_ws_id(vm).unwrap_or(0);
    ws_write_via(vm, id, chunk)
}

/// `ws.close()`：转发 underlyingSink.close / Node end。
fn ws_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_ws_id(vm).unwrap_or(0);
    ws_close_via(vm, id)
}

/// WritableStream 关闭统一路径。
fn ws_close_via(vm: &mut Vm, ws_id: u32) -> Result<Value, VmError> {
    let (node_sink, sink_close) =
        with_ws_state(ws_id, |s| (s.node_sink, s.sink_close)).unwrap_or((None, None));
    if let Some(node) = node_sink {
        let node_val = Value::Object(node);
        let _ = crate::builtins::stream::call_stream_method(vm, node_val, "end", &[])?;
    } else if let Some(c) = sink_close {
        vm.invoke_callable(c, Value::Undefined, &[])?;
    }
    resolved_promise(vm, Value::Undefined)
}

/// `writer.write(chunk)`：经 `_wsId` 关联所属流后走统一写入路径。
fn writer_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    let id = receiver_ws_id(vm).unwrap_or(0);
    ws_write_via(vm, id, chunk)
}

/// `writer.close()`：经 `_wsId` 关联所属流后走统一关闭路径。
fn writer_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let id = receiver_ws_id(vm).unwrap_or(0);
    ws_close_via(vm, id)
}

// --- ReadableStreamTee ------------------------------------------------------

/// `ReadableStreamTee([stream])`：优先调用 `stream.tee()`（以 stream 为 this）；
/// 无 `tee` 方法时回退 `[stream, stream]`；无参返回 undefined；非对象抛错
/// （逐条对齐 Go 的 ReadableStreamTee）。
fn readable_stream_tee(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(stream) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let Value::Object(_) = stream else {
        return Err(thrown(vm, "ReadableStreamTee: stream must be an object"));
    };
    let tee = vm.get_property(stream, "tee").unwrap_or(Value::Undefined);
    if is_function(vm, tee) {
        return vm.invoke_callable(tee, stream, &[]);
    }
    Ok(Value::Object(vm.alloc_array(vec![stream, stream])))
}

// --- M4 互通：跨模块工厂 ------------------------------------------------

/// M4 互通：为 `Readable.toWeb` 创建真实 ReadableStream 实例（无 underlyingSource）。
pub fn create_web_readable(vm: &mut Vm) -> Result<ObjectRef, VmError> {
    readable_stream_ctor(vm, &[]).map(|v| match v {
        Value::Object(r) => r,
        _ => unreachable!("readable_stream_ctor 恒返回对象"),
    })
}

/// M4 互通：Node 可写流写入转发目标（`Writable.fromWeb` 桥内部处理器用）。
pub fn web_sink_forward(vm: &mut Vm, ws_id: u32, chunk: Value) -> Result<Value, VmError> {
    ws_write_via(vm, ws_id, chunk)
}

// --- 模块注册 ----------------------------------------------------------------

/// 构建 `stream/web` 模块单例：三个构造器 + `ReadableStreamTee`。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    let rs_ctor = vm.alloc_native_ctor("ReadableStream", None);
    let ws_ctor = vm.alloc_native_ctor("WritableStream", None);
    let ts_ctor = vm.alloc_native_ctor("TransformStream", None);
    set_module_prop(vm, obj, "ReadableStream", Value::Object(rs_ctor))?;
    set_module_prop(vm, obj, "WritableStream", Value::Object(ws_ctor))?;
    set_module_prop(vm, obj, "TransformStream", Value::Object(ts_ctor))?;
    // `new ReadableStream(...)` 经 do_construct 按构造器名查分派表。
    registry
        .dispatch
        .insert("ReadableStream".to_owned(), readable_stream_ctor);
    registry
        .dispatch
        .insert("WritableStream".to_owned(), writable_stream_ctor);
    registry
        .dispatch
        .insert("TransformStream".to_owned(), transform_stream_ctor);

    let tee_fn = vm.alloc_native_fn("stream/web.ReadableStreamTee");
    set_module_prop(vm, obj, "ReadableStreamTee", Value::Object(tee_fn))?;
    register_handler(
        registry,
        "stream/web",
        "ReadableStreamTee",
        readable_stream_tee,
    );

    // 实例方法命名空间（经 `_builtinNs` 通用分派）。
    let ns_methods: &[(&str, &str, BuiltinHandler)] = &[
        (RS_NS, "getReader", rs_get_reader),
        (RS_NS, "tee", rs_tee),
        (RS_NS, "cancel", rs_cancel),
        (RS_NS, "enqueue", rs_enqueue),
        (RS_NS, "close", rs_close),
        (RS_NS, "pipeTo", rs_pipe_to),
        (READER_NS, "read", reader_read),
        (READER_NS, "cancel", reader_cancel),
        (READER_NS, "releaseLock", reader_release_lock),
        (WS_NS, "getWriter", ws_get_writer),
        (WS_NS, "write", ws_write),
        (WS_NS, "close", ws_close),
        (WRITER_NS, "write", writer_write),
        (WRITER_NS, "close", writer_close),
        (CTL_NS, "enqueue", rs_enqueue),
        (CTL_NS, "close", rs_close),
        (CTL_NS, "error", ctl_error),
    ];
    for (ns, method, handler) in ns_methods {
        register_handler(registry, ns, method, *handler);
    }

    Ok(obj)
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = readable_stream_ctor;
        let _: crate::builtins::BuiltinHandler = writable_stream_ctor;
        let _: crate::builtins::BuiltinHandler = transform_stream_ctor;
        let _: crate::builtins::BuiltinHandler = rs_get_reader;
        let _: crate::builtins::BuiltinHandler = rs_tee;
        let _: crate::builtins::BuiltinHandler = readable_stream_tee;
        let _: crate::builtins::BuiltinHandler = reader_read;
    }
}
