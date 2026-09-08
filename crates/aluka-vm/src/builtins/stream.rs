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
    /// 内部写队列：(chunk, 完成回调)——write_fn 串行逐块交付（Node 语义：
    /// 完成一个才启动下一个，队列清空时发 drain）
    write_queue: std::collections::VecDeque<(Value, Value)>,
    /// 写处理器状态：write_fn 在飞 / 完成回调已触发 / 处理器重入守卫
    write_busy: bool,
    write_cb_fired: bool,
    processing_writes: bool,
    /// `for await` 的 next 等待者：pending promise（按到达顺序兑现）
    awaiters: Vec<aluka_core::ObjectRef>,
    /// 水位线（字节）：writable_length 达到该值时 write 返回 false（M3.1）
    high_water_mark: usize,
    /// 已入队未完成写入的字节数（writableLength）
    writable_length: usize,
    /// 写入返回 false 后置位，drain 事件时清除（writableNeedDrain）
    need_drain: bool,
    /// 已销毁标记（destroy 后写入报错、事件不再触发）
    destroyed: bool,
    /// destroy(err) 存储的错误（errored 属性读取）
    errored: Option<Value>,
    /// pipe 背压暂停中：等待目标流 drain 事件恢复（防重复挂监听）
    pipe_wait_drain: bool,
    /// 自身句柄（内部 handler 发事件用；值由 JS 堆持有，GC 经闭包/监听器可达）
    self_handle: Option<Value>,
    /// drain_to_dest 重入守卫（同步完成回调触发的 drain 重入时置位）
    draining: bool,
    /// 可读缓冲字节长度（readableLength；push 入队/读取出队时维护）
    readable_length: usize,
}

/// 全局流状态存储表（对象句柄索引 -> 流内部状态）
static STREAM_STORE: Mutex<Option<HashMap<u32, StreamState>>> = Mutex::new(None);

/// 默认水位线（对齐 Node `stream` 默认 highWaterMark：64 KiB）
const DEFAULT_HIGH_WATER_MARK: usize = 64 * 1024;

/// 初始化流实例内部状态
fn init_stream_state(id: u32, write_fn: Option<Value>, high_water_mark: usize, self_val: Value) {
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
            write_queue: std::collections::VecDeque::new(),
            write_busy: false,
            write_cb_fired: false,
            processing_writes: false,
            awaiters: Vec::new(),
            high_water_mark,
            writable_length: 0,
            need_drain: false,
            destroyed: false,
            errored: None,
            pipe_wait_drain: false,
            self_handle: Some(self_val),
            draining: false,
            readable_length: 0,
        },
    );
}

/// 计算 chunk 的字节长度（Buffer/字符串取真实字节；其余类型 0）
fn chunk_byte_len(vm: &crate::interpreter::Vm, chunk: Value) -> usize {
    match chunk {
        Value::Object(_) => crate::builtins::buffer::extract_bytes(vm, chunk)
            .map(|b| b.len())
            .unwrap_or(0),
        _ => 0,
    }
}

/// 从 options 对象读取 highWaterMark（缺省/非法 → 默认水位线）
fn read_high_water_mark(vm: &mut crate::interpreter::Vm, args: &[Value]) -> usize {
    if let Some(Value::Object(opts)) = args.first().copied() {
        if let Ok(Value::Number(n)) = vm.get_property(Value::Object(opts), "highWaterMark") {
            if n.is_finite() && n >= 0.0 {
                return n as usize;
            }
        }
    }
    DEFAULT_HIGH_WATER_MARK
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

/// M4 互通桥登记表：Node 可读流 id → web ReadableStream id（`Readable.toWeb`）。
static WEB_BRIDGES: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// M4 互通（stream_web.rs → Node 侧）：fromWeb 桥转发——把 web 侧 chunk
/// 经 Node `push` 语义写入可读流（Null 哨兵同样适用）。
pub(crate) fn node_bridge_push(
    vm: &mut crate::interpreter::Vm,
    node_id: u32,
    chunk: Value,
) -> Result<(), crate::interpreter::VmError> {
    call_stream_method(
        vm,
        Value::Object(aluka_core::ObjectRef(node_id)),
        "push",
        &[chunk],
    )?;
    Ok(())
}

/// M4 互通：登记 toWeb 桥（Node 可读流 id → web 流 id）。
fn attach_web_bridge(node_id: u32, web_id: u32) {
    let mut guard = WEB_BRIDGES.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(node_id, web_id);
}

/// M4 互通：Node push 数据/关闭哨兵实时转发到 web 流（`Readable.toWeb` live 桥）。
fn forward_to_web(id: u32, chunk: Value) {
    let web_id = WEB_BRIDGES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&id).copied());
    if let Some(web_id) = web_id {
        if matches!(chunk, Value::Null) {
            crate::builtins::stream_web::web_bridge_close(web_id);
        } else {
            crate::builtins::stream_web::web_bridge_enqueue(web_id, chunk);
        }
    }
}

/// `Readable.fromWeb(webStream)`：把 web ReadableStream 桥接为 Node 可读流。
/// 登记双向 live 桥后，web 侧 `controller.enqueue` / `close` 同步转发 Node 侧
/// `push` / `push(null)`（Node 22 LTS 对齐面）。
fn readable_from_web(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(web) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let web_id = match &web {
        Value::Object(r) => r.0,
        _ => {
            let msg =
                vm.alloc_string("Readable.fromWeb: stream must be a ReadableStream".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
    };
    let node = create_readable_instance(vm, &[])?;
    crate::builtins::stream_web::attach_node_bridge_and_drain(vm, web_id, node.0)?;
    Ok(Value::Object(node))
}

/// `Readable.toWeb(nodeReadable)`：把 Node 可读流桥接为 web ReadableStream。
/// 既有缓冲立即转入 web 队列（已结束则同步关闭），此后 Node `push` 经
/// live 桥实时入队。
fn readable_to_web(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(node_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let node_id = match node_val {
        Value::Object(r) => r.0,
        _ => {
            let msg = vm.alloc_string("Readable.toWeb: stream must be a Readable".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
    };
    let web = crate::builtins::stream_web::create_web_readable(vm)?;
    let state = get_stream_state(node_id);
    for chunk in &state.buffer {
        crate::builtins::stream_web::web_bridge_enqueue(web.0, *chunk);
    }
    if state.ended {
        crate::builtins::stream_web::web_bridge_close(web.0);
    }
    attach_web_bridge(node_id, web.0);
    Ok(Value::Object(web))
}

/// `Writable.fromWeb(webStream)`：把 web WritableStream 桥接为 Node 可写流。
/// Node 侧 `write` 经内部处理器转发 web 侧 underlyingSink.write。
fn writable_from_web(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(web) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let web_id = match &web {
        Value::Object(r) => r.0,
        _ => {
            let msg =
                vm.alloc_string("Writable.fromWeb: stream must be a WritableStream".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
    };
    let fwd = vm.alloc_native_fn("stream:internal.webSinkWrite");
    vm.set_native_fn_property(fwd, "_webId", Value::Number(web_id as f64));
    let opts = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(opts), "write", Value::Object(fwd));
    let node = create_writable_instance(vm, &[Value::Object(opts)])?;
    Ok(Value::Object(node))
}

/// `Writable.toWeb(nodeWritable)`：把 Node 可写流桥接为 web WritableStream。
/// web 侧 writer.write / close 转发 Node write / end。
fn writable_to_web(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(node_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let node = match node_val {
        Value::Object(r) => r,
        _ => {
            let msg = vm.alloc_string("Writable.toWeb: stream must be a Writable".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
    };
    let web = crate::builtins::stream_web::create_web_writable(vm)?;
    crate::builtins::stream_web::attach_node_sink(web.0, node);
    Ok(Value::Object(web))
}

/// webSinkWrite 内部处理器：fromWeb 桥的 Node write_fn → web underlyingSink.write。
fn stream_internal_web_sink_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(fwd_ref) = super::pending_callee() else {
        return Ok(Value::Undefined);
    };
    let Some(Value::Number(n)) = vm.get_native_fn_property(fwd_ref, "_webId") else {
        return Ok(Value::Undefined);
    };
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    crate::builtins::stream_web::web_sink_forward(vm, n as u32, chunk)
}

/// GC 根提供者：STREAM_STORE 持有的全部堆值（缓冲 chunk、写队列、监听器、
/// pipe 目标、等待者 promise 等——静态表在 JS 可达图之外，必须显式登记）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    let guard = STREAM_STORE.lock().unwrap();
    let Some(map) = guard.as_ref() else {
        return;
    };
    for s in map.values() {
        for v in &s.buffer {
            out.push(*v);
        }
        for (chunk, cb) in &s.write_queue {
            out.push(*chunk);
            out.push(*cb);
        }
        if let Some(w) = s.write_fn {
            out.push(w);
        }
        if let Some(p) = s.pipe_dest {
            out.push(p);
        }
        for cbs in s.listeners.values() {
            for cb in cbs {
                out.push(*cb);
            }
        }
        for a in &s.awaiters {
            out.push(Value::Object(*a));
        }
        if let Some(h) = s.self_handle {
            out.push(h);
        }
        if let Some(e) = s.errored {
            out.push(e);
        }
    }
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

/// 排空缓冲区到管道目标流（背压联动：dest.write 返回 false 时暂停源流，
/// 挂 drain 监听待目标流恢复后继续排空——M3.1 背压联动核心）。
/// 写前先出队（防同步完成回调触发的重入 drain 重复写同一块）。
fn drain_to_dest(vm: &mut Vm, id: u32, stream_val: Value, dest: Value) -> Result<(), VmError> {
    // 重入守卫：同步 cb → drain 事件 → 本函数重入时，外层循环持有控制权
    let reentered = with_stream_state(id, |s| {
        let was = s.draining;
        s.draining = true;
        was
    })
    .unwrap_or(true);
    if reentered {
        return Ok(());
    }
    let result = loop {
        let front = with_stream_state(id, |s| s.buffer.first().copied()).flatten();
        let Some(chunk) = front else {
            break Ok(());
        };
        if matches!(chunk, Value::Null) {
            with_stream_state(id, |s| {
                s.buffer.remove(0);
                s.readable_length = s.readable_length.saturating_sub(0);
            });
            finish_readable(vm, id, stream_val)?;
            end_pipe_dest(vm, dest)?;
            break Ok(());
        }
        // 写前出队：chunk 已交付 write_fn（背压仅意味着"暂停推入"，
        // 不回队——回队会导致该块二次交付）
        with_stream_state(id, |s| {
            s.buffer.remove(0);
            let blen = chunk_byte_len(vm, chunk);
            s.readable_length = s.readable_length.saturating_sub(blen);
        });
        let ok = write_to_stream(vm, dest, chunk)?;
        if matches!(ok, Value::Boolean(false)) {
            // 目标流背压：源流暂停（剩余块留在缓冲），等 dest 'drain' 后恢复
            with_stream_state(id, |s| {
                s.flowing = false;
                s.pipe_wait_drain = true;
            });
            attach_pipe_drain_listener(vm, stream_val)?;
            break Ok(());
        }
    };
    with_stream_state(id, |s| s.draining = false);
    result
}

/// 为 pipe 背压挂 `drain` 恢复监听（去重：源流 pipe_wait_drain 已置位时
/// 由调用方保证只挂一次）。
fn attach_pipe_drain_listener(vm: &mut Vm, src_val: Value) -> Result<(), VmError> {
    let listener = vm.alloc_native_fn("stream:internal.pipeDrain");
    vm.set_native_fn_property(listener, "_src", src_val);
    let drain_str = Value::Object(vm.alloc_string("drain".to_owned()));
    stream_on(vm, &[drain_str, Value::Object(listener)])?;
    Ok(())
}

/// pipe 背压恢复 handler：dest 'drain' 后恢复源流排空。
fn stream_internal_pipe_drain(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let callee = super::pending_callee();
    let src_val = match callee {
        Value::Object(r) => vm
            .get_native_fn_property(r, "_src")
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    };
    let Value::Object(r) = src_val else {
        return Ok(Value::Undefined);
    };
    let dest = with_stream_state(r.0, |s| {
        s.pipe_wait_drain = false;
        s.flowing = true;
        s.pipe_dest
    })
    .unwrap_or(None);
    if let Some(d) = dest {
        // 先排空剩余缓冲（push(null) 的 Null 哨兵在队尾时按序收尾）
        drain_to_dest(vm, r.0, src_val, d)?;
    }
    let (ended, buf_empty) =
        with_stream_state(r.0, |s| (s.ended, s.buffer.is_empty())).unwrap_or((false, true));
    if ended && buf_empty {
        finish_readable(vm, r.0, src_val)?;
        if let Some(d) = dest {
            end_pipe_dest(vm, d)?;
        }
    }
    Ok(Value::Undefined)
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
pub(crate) fn call_stream_method(
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
pub fn create_readable_instance(vm: &mut Vm, args: &[Value]) -> Result<ObjectRef, VmError> {
    // Ordinary 实例（与 Writable 同构）：方法经注册表分派；堆 Readable 变体
    // 保留给 fs/http 等原生物流的快速路径
    let obj = vm.alloc_ordinary();
    let hwm = read_high_water_mark(vm, args);
    let self_val = Value::Object(obj);
    init_stream_state(obj.0, None, hwm, self_val);

    let _ = vm.set_property(Value::Object(obj), "_isStream", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(obj), "_isReadable", Value::Boolean(true));
    let _ = vm.set_property(
        Value::Object(obj),
        "readableHighWaterMark",
        Value::Number(hwm as f64),
    );

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
    let hwm = read_high_water_mark(vm, args);
    let self_val = Value::Object(obj);
    let mut write_fn = None;
    if let Some(Value::Object(opts_ref)) = args.first() {
        if let Ok(w) = vm.get_property(Value::Object(*opts_ref), "write") {
            if matches!(w, Value::Object(_)) {
                write_fn = Some(w);
            }
        }
    }
    init_stream_state(obj.0, write_fn, hwm, self_val);

    let _ = vm.set_property(Value::Object(obj), "_isStream", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(obj), "_isWritable", Value::Boolean(true));
    let _ = vm.set_property(
        Value::Object(obj),
        "writableHighWaterMark",
        Value::Number(hwm as f64),
    );

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
    // M4 互通：Readable.fromWeb / Readable.toWeb
    let from_web_fn = vm.alloc_native_fn("stream.Readable.fromWeb");
    let _ = vm.set_property(
        Value::Object(readable_ctor),
        "fromWeb",
        Value::Object(from_web_fn),
    );
    let r_to_web_fn = vm.alloc_native_fn("stream.Readable.toWeb");
    let _ = vm.set_property(
        Value::Object(readable_ctor),
        "toWeb",
        Value::Object(r_to_web_fn),
    );

    // Writable 构造器对象
    let writable_ctor = vm.alloc_native_fn("stream.Writable");
    // M4 互通：Writable.fromWeb / Writable.toWeb
    let w_from_web_fn = vm.alloc_native_fn("stream.Writable.fromWeb");
    let _ = vm.set_property(
        Value::Object(writable_ctor),
        "fromWeb",
        Value::Object(w_from_web_fn),
    );
    let w_to_web_fn = vm.alloc_native_fn("stream.Writable.toWeb");
    let _ = vm.set_property(
        Value::Object(writable_ctor),
        "toWeb",
        Value::Object(w_to_web_fn),
    );

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
        register_handler(registry, ns, "fromWeb", readable_from_web);
        register_handler(registry, ns, "toWeb", readable_to_web);
    }
    // Writable 侧互通（键与 NativeFn 名对齐：stream.Writable.fromWeb / toWeb）
    register_handler(registry, "stream.Writable", "fromWeb", writable_from_web);
    register_handler(registry, "stream.Writable", "toWeb", writable_to_web);
    // Readable 静态互通键（属性调用 Readable.fromWeb(x) / 裸调用皆经此分派）
    register_handler(registry, "stream.Readable", "fromWeb", readable_from_web);
    register_handler(registry, "stream.Readable", "toWeb", readable_to_web);
    // `Readable.from(x)` 属性调用形态键（缺它时形态一回退原构造器——静默建空流）
    register_handler(registry, "stream.Readable", "from", readable_from);

    // 内部 handler（write 完成回调 / pipe 背压恢复 / pipeline 错误级联）
    register_handler(
        registry,
        "stream:internal",
        "writeCb",
        stream_internal_write_cb,
    );
    register_handler(
        registry,
        "stream:internal",
        "pipeDrain",
        stream_internal_pipe_drain,
    );
    register_handler(
        registry,
        "stream:internal",
        "pipelineError",
        stream_internal_pipeline_error,
    );
    register_handler(
        registry,
        "stream:internal",
        "webSinkWrite",
        stream_internal_web_sink_write,
    );

    Ok(obj)
}

/// 流实例计算属性（`writableLength`/`writableNeedDrain`/`destroyed` 等；
/// get_property 对挂 `_isStream` 标记的对象路由到此处）。
pub(crate) fn stream_computed_prop(id: u32, key: &str) -> Option<Value> {
    let s = get_stream_state(id);
    match key {
        "writableLength" => Some(Value::Number(s.writable_length as f64)),
        "writableHighWaterMark" => Some(Value::Number(s.high_water_mark as f64)),
        "readableHighWaterMark" => Some(Value::Number(s.high_water_mark as f64)),
        "readableLength" => Some(Value::Number(s.buffer.len() as f64)),
        "writableNeedDrain" => Some(Value::Boolean(s.need_drain)),
        "destroyed" => Some(Value::Boolean(s.destroyed)),
        "flowing" => Some(Value::Boolean(s.flowing)),
        "errored" => Some(s.errored.unwrap_or(Value::Null)),
        _ => None,
    }
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
    if with_stream_state(id, |s| s.destroyed).unwrap_or(false) {
        return Ok(Value::Boolean(false));
    }
    // M4 toWeb 桥：数据与结束哨兵实时转发到 web 流队列
    if let Some(chunk) = args.first().copied() {
        forward_to_web(id, chunk);
    }
    if let Some(chunk) = args.first().copied() {
        if matches!(chunk, Value::Null) {
            with_stream_state(id, |s| s.ended = true);
            // pipe 背压暂停中：Null 哨兵入队，恢复排空后按序收尾
            let paused = with_stream_state(id, |s| {
                if s.pipe_wait_drain {
                    s.buffer.push(Value::Null);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            let state = get_stream_state(id);
            // 等待中的 for await next：立即以 done 兑现
            fulfill_next_awaiter(vm, id, Value::Undefined, true)?;
            if state.flowing && !paused {
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
                let blen = chunk_byte_len(vm, chunk);
                with_stream_state(id, |s| {
                    s.buffer.push(chunk);
                    s.readable_length += blen;
                });
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
fn stream_read(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Null),
    };
    let chunk = with_stream_state(id, |s| {
        if !s.buffer.is_empty() {
            let c = s.buffer.remove(0);
            let blen = chunk_byte_len(vm, c);
            s.readable_length = s.readable_length.saturating_sub(blen);
            Some(c)
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
    // 背压联动：dest 队列清空发 'drain' 时恢复源流排空（Node pipe 语义）
    attach_pipe_drain_listener(vm, receiver)?;
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

/// `stream.destroy([error])`：幂等销毁——置 destroyed、清缓冲、发
/// error（携带 err 时）与 close；错误存 errored 供 `stream.errored` 读取。
fn stream_destroy(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    if let Value::Object(r) = receiver {
        let already = with_stream_state(r.0, |s| {
            let was = s.destroyed;
            s.destroyed = true;
            s.ended = true;
            s.buffer.clear();
            s.readable_length = 0;
            s.writable_length = 0;
            was
        })
        .unwrap_or(true);
        if already {
            return Ok(receiver);
        }
        if let Some(err) = args.first() {
            if !matches!(err, Value::Undefined | Value::Null) {
                with_stream_state(r.0, |s| s.errored = Some(*err));
                emit_event(vm, receiver, "error", &[*err])?;
            }
        }
        emit_event(vm, receiver, "close", &[])?;
    }
    Ok(receiver)
}

/// `stream.write(chunk[, encoding][, callback])`：向可写流写入数据。
///
/// 背压语义（M3.1）：入队后 `writable_length` 达到水位线 → 返回 `false`
/// 并置 `writableNeedDrain`；write_fn 完成回调（cb）触发时扣减队列长度，
/// 跌破水位线 → 发 `drain` 事件恢复上游。
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
    if state.destroyed {
        let msg = vm.alloc_string("write after destroy".to_owned());
        return Err(VmError::Thrown(Value::Object(msg)));
    }
    if state.ended {
        let err = vm.alloc_error_instance("write after end");
        let name = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        emit_event(vm, receiver, "error", &[Value::Object(err)])?;
        return Ok(Value::Boolean(false));
    }
    let len = chunk_byte_len(vm, chunk);
    if state.write_fn.is_some() {
        // 入内部写队列：串行逐块交付（完成一个启动下一个）；
        // writable_length 达到水位线 → 返回 false（背压信号）
        let cb = make_write_callback(vm, id, len);
        with_stream_state(id, |s| {
            s.write_queue.push_back((chunk, cb));
            s.writable_length += len;
        });
        process_write_queue(vm, id)?;
    } else {
        with_stream_state(id, |s| {
            s.buffer.push(chunk);
            s.writable_length += len;
        });
    }
    let state = get_stream_state(id);
    if state.writable_length >= state.high_water_mark {
        with_stream_state(id, |s| s.need_drain = true);
        return Ok(Value::Boolean(false));
    }
    Ok(Value::Boolean(true))
}

/// 写队列处理器：串行将队头交付 write_fn；同步完成（write_fn 内直接调 cb）
/// 时在循环内继续下一块，异步完成（cb 由调度器触发）时返回、由 writeCb
/// handler 重新进入。processing_writes 防同步链重入。
fn process_write_queue(vm: &mut Vm, id: u32) -> Result<(), VmError> {
    let reentered = with_stream_state(id, |s| {
        let was = s.processing_writes;
        s.processing_writes = true;
        was
    })
    .unwrap_or(true);
    if reentered {
        return Ok(());
    }
    loop {
        // 完成回调检测：write_fn 已同步/异步完成 → 释放在飞标记
        if with_stream_state(id, |s| s.write_cb_fired).unwrap_or(false) {
            with_stream_state(id, |s| {
                s.write_cb_fired = false;
                s.write_busy = false;
                s.write_queue.pop_front();
            });
        }
        let (busy, empty, need) = with_stream_state(id, |s| {
            (s.write_busy, s.write_queue.is_empty(), s.need_drain)
        })
        .unwrap_or((true, true, false));
        if busy {
            break; // 异步在飞：等 writeCb 重新进入
        }
        if empty {
            // 队列清空：need_drain 置位时发 drain 恢复上游（drain 监听器
            // 可能同步入队新写——continue 继续处理而非退出）
            if need {
                with_stream_state(id, |s| s.need_drain = false);
                let self_val = with_stream_state(id, |s| s.self_handle).flatten();
                if let Some(sv) = self_val {
                    emit_event(vm, sv, "drain", &[])?;
                }
                continue;
            }
            break;
        }
        let (chunk, cb) = with_stream_state(id, |s| {
            s.write_queue
                .front()
                .cloned()
                .unwrap_or((Value::Undefined, Value::Undefined))
        })
        .unwrap_or((Value::Undefined, Value::Undefined));
        with_stream_state(id, |s| {
            s.write_busy = true;
            s.write_cb_fired = false;
        });
        let write_fn = with_stream_state(id, |s| s.write_fn).flatten();
        let Some(write_fn) = write_fn else {
            with_stream_state(id, |s| {
                s.write_queue.clear();
                s.write_busy = false;
            });
            break;
        };
        let empty_str = Value::Object(vm.alloc_string(String::new()));
        vm.invoke_callable(
            write_fn,
            Value::Object(aluka_core::ObjectRef(id)),
            &[chunk, empty_str, cb],
        )?;
        // write_fn 未调 cb → 同步完成，自动出队继续下一块
        //（简化 Writable 的 write 不带 cb 参数时的兼容路径）
        if !with_stream_state(id, |s| s.write_cb_fired).unwrap_or(false) {
            with_stream_state(id, |s| {
                s.write_busy = false;
                s.write_queue.pop_front();
            });
        }
    }
    with_stream_state(id, |s| s.processing_writes = false);
    Ok(())
}

/// 构造 write_fn 的完成回调（NativeFn：属性携带流句柄与本块字节数）。
fn make_write_callback(vm: &mut Vm, stream_id: u32, chunk_len: usize) -> Value {
    let cb = vm.alloc_native_fn("stream:internal.writeCb");
    vm.set_native_fn_property(cb, "_sid", Value::Number(stream_id as f64));
    vm.set_native_fn_property(cb, "_clen", Value::Number(chunk_len as f64));
    Value::Object(cb)
}

/// `write` 完成回调 handler：扣减 writable_length，跌破水位线发 `drain`。
fn stream_internal_write_cb(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(cb_ref) = super::pending_callee() else {
        return Ok(Value::Undefined);
    };
    let sid = match vm.get_native_fn_property(cb_ref, "_sid") {
        Some(Value::Number(n)) => n as u32,
        _ => return Ok(Value::Undefined),
    };
    let clen = match vm.get_native_fn_property(cb_ref, "_clen") {
        Some(Value::Number(n)) => n as usize,
        _ => 0,
    };
    // 回调携带错误 → 存 errored、级联销毁流并触发 error 事件
    if let Some(err) = args.first().copied() {
        if !matches!(err, Value::Undefined | Value::Null) {
            with_stream_state(sid, |s| {
                s.errored = Some(err);
                s.destroyed = true;
            });
            emit_event(
                vm,
                Value::Object(aluka_core::ObjectRef(sid)),
                "error",
                &[err],
            )?;
            return Ok(Value::Undefined);
        }
    }
    with_stream_state(sid, |s| {
        s.writable_length = s.writable_length.saturating_sub(clen);
        s.write_cb_fired = true;
    });
    process_write_queue(vm, sid)?;
    Ok(Value::Undefined)
}

/// `stream.end([chunk][, callback])`：结束可写流
fn stream_end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let id = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    if with_stream_state(id, |s| s.destroyed).unwrap_or(false) {
        return Ok(receiver);
    }
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

    // 错误级联（M3.1）：任一流 error → 销毁其余全部流（destroy(err)），
    // 回调以该错误调用一次（_fired 标记去重）
    for (i, sv) in streams.iter().enumerate() {
        let handler = vm.alloc_native_fn("stream:internal.pipelineError");
        vm.set_native_fn_property(handler, "_self", *sv);
        let arr = vm.alloc_array(streams.to_vec());
        vm.set_native_fn_property(handler, "_all", Value::Object(arr));
        vm.set_native_fn_property(handler, "_idx", Value::Number(i as f64));
        if let Some(cbv) = cb {
            vm.set_native_fn_property(handler, "_cb", cbv);
        }
        let err_str = Value::Object(vm.alloc_string("error".to_owned()));
        let _ = call_stream_method(vm, *sv, "on", &[err_str, Value::Object(handler)])?;
    }

    let mut current = streams[0];
    for next_stream in streams.iter().skip(1).copied() {
        current = call_stream_method(vm, current, "pipe", &[next_stream])?;
    }

    Ok(streams[streams.len() - 1])
}

/// pipeline 错误级联 handler：销毁除本流外的全部流（destroy(err)），回调
/// 以同一错误调用一次（_fired 去重，防多流同时报错重复回调）。
fn stream_internal_pipeline_error(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(cb_ref) = super::pending_callee() else {
        return Ok(Value::Undefined);
    };
    let fired = matches!(
        vm.get_native_fn_property(cb_ref, "_fired"),
        Some(Value::Boolean(true))
    );
    if fired {
        return Ok(Value::Undefined);
    }
    vm.set_native_fn_property(cb_ref, "_fired", Value::Boolean(true));
    let self_val = vm
        .get_native_fn_property(cb_ref, "_self")
        .unwrap_or(Value::Undefined);
    let all = vm.get_native_fn_property(cb_ref, "_all");
    let err = args.first().copied().unwrap_or(Value::Undefined);
    if let Some(Value::Object(arr)) = all
        && let Some(HeapObject::Array { elements, .. }) = vm.heap.get(arr.0 as usize)
    {
        for sv in elements.clone() {
            if sv == self_val {
                continue;
            }
            let _ = call_stream_method(vm, sv, "destroy", std::slice::from_ref(&err))?;
        }
    }
    if let Some(cb) = vm.get_native_fn_property(cb_ref, "_cb") {
        vm.invoke_callable(cb, Value::Undefined, &[err])?;
    }
    Ok(Value::Undefined)
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
