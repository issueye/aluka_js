//! 内置库注册表：把内置模块的纯函数实现聚合为可独立并行开发的单元。
//!
//! 并行开发纪律（see `aluka_r/docs/builtins-plan.md`）：
//! - 每个模块一个独立文件（如 `builtins/querystring.rs`），实现一个
//!   [`ModuleBuilder`] 注册函数；
//! - **禁止修改** `interpreter.rs` / `call.rs` / `property.rs` 等核心解释器
//!   文件——本注册表经一道通用分派分支接入解释器（`CALL_METHOD` 时查找）；
//! - 方法以 `模块名.方法名` 命名（如 `querystring.parse`），native 属性
//!   （如 `os.EOL`）走模块对象属性物化（模块创建时写入）。
//!
//! 分派模型：`CALL_METHOD` 拦截链在 receiver 是模块单例且方法名命中
//! [`BuiltinRegistry::lookup`] 时，调 [`BuiltinHandler`]（纯函数指针，
//! 无借用问题），返回值直接压栈。

pub mod assert;
pub mod assert_strict;
pub mod async_hooks;
pub mod broadcast_channel;
pub mod buffer;
pub mod child_process;
pub mod cluster;
pub mod constants;
pub mod crypto;
pub mod dgram;
pub mod diagnostics_channel;
pub mod dns;
pub mod dns_promises;
pub mod domain;
pub mod events;
pub mod fs;
pub mod fs_promises;
pub mod global_fns;
pub mod http;
pub mod http2;
pub mod https;
pub mod inspector;
pub mod inspector_promises;
pub mod markdown;
pub mod module;
pub mod net;
pub mod os;
pub mod path_posix;
pub mod path_win32;
pub mod perf_hooks;
pub mod promise;
pub mod punycode;
pub mod querystring;
pub mod readline;
pub mod readline_promises;
pub mod reflect;
pub mod repl;
pub mod require_aliases;
pub mod sqlite;
pub mod stream;
pub mod stream_web;
pub mod string_decoder;
pub mod surface;
pub mod sys;
pub mod test;
pub mod test_reporters;
pub mod timers;
pub mod tls;
pub mod trace_events;
pub mod tty;
pub mod util;
pub mod v8;
pub mod vm;
pub mod wasi;
pub mod worker_threads;
pub mod zlib;

pub(crate) use crate::microtask::{Job, PendingResume};

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CURRENT_RECEIVER: RefCell<Value> = const { RefCell::new(Value::Undefined) };
}

/// 内置库事件源泵签名：轮询一次 I/O 事件源（socket / 子进程 / 线程消息等），
/// 返回 `Ok(true)` 表示本轮有进展（派发了回调 / 产生了事件），事件循环据此
/// 决定是否继续泵询。
pub type EventSourcePump = fn(&mut Vm) -> Result<bool, VmError>;

impl Vm {
    /// 注册并激活内置库事件源（随 `Vm` 实例生命周期，不跨运行泄漏）；
    /// 同名重复注册幂等（更新泵函数并保持活跃）。
    pub fn activate_event_source(&mut self, name: &'static str, pump: EventSourcePump) {
        if let Some(entry) = self.event_sources.iter_mut().find(|(n, _)| *n == name) {
            entry.1 = pump;
        } else {
            self.event_sources.push((name, pump));
        }
    }

    /// 注销事件源：如 `server.close()` 后调用，事件循环不再为其泵询。
    pub fn deactivate_event_source(&mut self, name: &str) {
        self.event_sources.retain(|(n, _)| *n != name);
    }

    /// 是否存在活跃事件源（顶层事件循环据此决定是否继续泵询）。
    #[must_use]
    pub fn has_active_event_sources(&self) -> bool {
        !self.event_sources.is_empty()
    }

    /// 泵一轮全部活跃事件源；返回是否有任一源报告进展。
    pub(crate) fn pump_event_sources(&mut self) -> Result<bool, VmError> {
        let pumps: Vec<EventSourcePump> =
            self.event_sources.iter().map(|(_, pump)| *pump).collect();
        let mut progressed = false;
        for pump in pumps {
            if pump(self)? {
                progressed = true;
            }
        }
        Ok(progressed)
    }
}

/// 读取实例对象上的 `_builtinNs` 命名空间标记（堆字符串），用于通用实例分派：
/// 内置库的动态实例（如 `crypto` 的 Hash 实例）把 `_builtinNs` 设为
/// `"crypto:hash"` 之类的命名空间串，`CALL_METHOD` 即按 `"{ns}.{method}"`
/// 查分派表，无需修改 [`try_dispatch`]。
fn builtin_ns(vm: &Vm, r: ObjectRef) -> Option<String> {
    let v = vm.own_value(r.index(), "_builtinNs")?;
    match v {
        Value::Object(s) => match vm.heap.get(s.index()) {
            Some(HeapObject::String(text)) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// 设置当前分派调用的接收者（this）。
pub fn set_current_receiver(v: Value) {
    CURRENT_RECEIVER.with(|r| *r.borrow_mut() = v);
}

/// 获取当前分派调用的接收者（this）。
pub fn current_receiver() -> Value {
    CURRENT_RECEIVER.with(|r| *r.borrow())
}

thread_local! {
    /// 当前分派调用的 NativeFn 全名（同一 handler 注册多键时区分方法）
    static PENDING_NATIVE_NAME: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 记录当前分派调用的 NativeFn 全名（handler 分派前设置）。
pub fn set_pending_native_name(name: &str) {
    PENDING_NATIVE_NAME.with(|n| *n.borrow_mut() = name.to_owned());
}

/// 读取当前分派调用的 NativeFn 全名。
pub fn pending_native_name() -> String {
    PENDING_NATIVE_NAME.with(|n| n.borrow().clone())
}

/// 内置方法处理器：`(vm, 实参) -> 返回值`。
pub type BuiltinHandler = fn(&mut Vm, &[Value]) -> Result<Value, VmError>;

/// 内置模块注册条目。
pub struct ModuleDef {
    /// 模块名（`require("name")` 与 `node:name` 命中）
    pub name: &'static str,
    /// 创建模块对象并登记方法到 `registry`（返回模块单例句柄）
    ///
    /// # Errors
    /// 实现失败时返回 VM 错误（正常实现不应失败）
    pub build: fn(&mut Vm, &mut BuiltinRegistry) -> Result<ObjectRef, VmError>,
}

/// 内置库注册表：模块对象工厂 + 方法名到处理器的分派表。
#[derive(Debug, Default)]
pub struct BuiltinRegistry {
    /// `模块名.方法名` → 处理器
    dispatch: HashMap<String, BuiltinHandler>,
    /// 模块单例句柄（模块名 → 堆句柄）
    modules: HashMap<&'static str, ObjectRef>,
}

/// 内置模块清单（新增模块在 `mod.rs` 内注册）。
macro_rules! builtin_modules {
    () => {
        &[
            crate::builtins::global_fns::MODULE,
            crate::builtins::constants::MODULE,
            crate::builtins::path_posix::MODULE,
            crate::builtins::path_win32::MODULE,
            crate::builtins::querystring::MODULE,
            crate::builtins::string_decoder::MODULE,
            crate::builtins::fs::MODULE,
            crate::builtins::fs::STAT_MODULE,
            crate::builtins::os::MODULE,
            crate::builtins::util::MODULE,
            crate::builtins::util::TYPES_MODULE,
            crate::builtins::assert::MODULE,
            crate::builtins::buffer::MODULE,
            crate::builtins::buffer::BUFFER_CLASS_MODULE,
            crate::builtins::buffer::INSTANCE_MODULE,
            crate::builtins::perf_hooks::MODULE,
            crate::builtins::perf_hooks::PERFORMANCE_MODULE,
            crate::builtins::v8::MODULE,
            crate::builtins::timers::MODULE,
            crate::builtins::timers::PROMISES_MODULE,
            crate::builtins::assert_strict::MODULE,
            crate::builtins::sys::MODULE,
            crate::builtins::fs_promises::MODULE,
            crate::builtins::events::MODULE,
            crate::builtins::events::EMITTER_CLASS_MODULE,
            crate::builtins::events::INSTANCE_MODULE,
            crate::builtins::stream::MODULE,
            crate::builtins::stream::PROMISES_MODULE,
            crate::builtins::stream::CONSUMERS_MODULE,
            crate::builtins::zlib::MODULE,
            crate::builtins::stream_web::MODULE,
            crate::builtins::crypto::MODULE,
            crate::builtins::child_process::MODULE,
            crate::builtins::worker_threads::MODULE,
            crate::builtins::cluster::MODULE,
            crate::builtins::vm::MODULE,
            crate::builtins::module::MODULE,
            crate::builtins::module::MODULE_CLASS,
            crate::builtins::trace_events::MODULE,
            crate::builtins::readline::MODULE,
            crate::builtins::readline_promises::MODULE,
            crate::builtins::repl::MODULE,
            crate::builtins::tty::MODULE,
            crate::builtins::sqlite::MODULE,
            crate::builtins::punycode::MODULE,
            crate::builtins::wasi::MODULE,
            crate::builtins::test::MODULE,
            crate::builtins::test_reporters::MODULE,
            crate::builtins::markdown::MODULE,
            crate::builtins::markdown::ALUKA_MODULE,
            crate::builtins::diagnostics_channel::MODULE,
            crate::builtins::async_hooks::MODULE,
            crate::builtins::inspector::MODULE,
            crate::builtins::inspector_promises::MODULE,
            crate::builtins::domain::MODULE,
            crate::builtins::http::MODULE,
            crate::builtins::https::MODULE,
            crate::builtins::http2::MODULE,
            crate::builtins::net::MODULE,
            crate::builtins::dns::MODULE,
            crate::builtins::dns_promises::MODULE,
            crate::builtins::dgram::MODULE,
            crate::builtins::tls::MODULE,
            crate::builtins::require_aliases::PROCESS_MODULE,
            crate::builtins::require_aliases::CONSOLE_MODULE,
            crate::builtins::require_aliases::URL_MODULE,
        ]
    };
}

/// 向解释器注册内置库：调用全部模块的 `build` 并预热注册表。
///
/// 在 `Vm::new` 中调用一次；模块可多次 `require`（命中各自单例）。
pub fn register_all(vm: &mut Vm) -> Result<(), VmError> {
    let defs: &[ModuleDef] = builtin_modules!();
    let mut registry = BuiltinRegistry::default();
    for def in defs {
        let module_ref = (def.build)(vm, &mut registry)?;
        registry.modules.insert(def.name, module_ref);
    }
    // 原型方法面（属性挂载 + Function.prototype.toString handler）
    crate::builtins::surface::register_surface(vm, &mut registry);
    vm.builtin_registry = registry;
    Ok(())
}

/// 模块注册表便捷宏：声明模块与方法的处理器映射。
///
/// 用法（模块文件内）：
/// ```ignore
/// use crate::builtins::{register_all, BuiltinRegistry};
/// pub const MODULE: ModuleDef = ModuleDef {
///     name: "querystring",
///     build,
/// };
/// fn build(vm: &mut Vm) -> Result<ObjectRef, VmError> { /* 建对象+登记 */ }
/// ```
#[macro_export]
macro_rules! builtin_module {
    ($name:literal, $build:path, $handlers:expr) => {
        pub const MODULE: $crate::builtins::ModuleDef = $crate::builtins::ModuleDef {
            name: $name,
            build: $build,
        };
    };
}

/// 注册模块时把方法名登记进分派表（模块 build 内部调用）。
pub fn register_handler(
    registry: &mut BuiltinRegistry,
    module: &str,
    method: &str,
    handler: BuiltinHandler,
) {
    registry.dispatch.insert(join_key(module, method), handler);
}

fn join_key(module: &str, method: &str) -> String {
    format!("{module}.{method}")
}

impl BuiltinRegistry {
    /// 以「模块名.方法名」`full` 查询分派表。
    #[must_use]
    pub fn lookup(&self, full: &str) -> Option<BuiltinHandler> {
        self.dispatch.get(full).copied()
    }

    /// 模块单例句柄。
    #[must_use]
    pub fn module(&self, name: &str) -> Option<ObjectRef> {
        self.modules.get(name).copied()
    }

    /// 注册既有堆对象为具名模块对象（`register_all` 之外的补挂场景，
    /// 如 Proxy 构造器静态面），供 `try_dispatch` 形态二反查分派键。
    pub fn register_module_object(&mut self, key: &'static str, r: ObjectRef) {
        self.modules.insert(key, r);
    }

    /// 反查句柄所属模块名。
    #[must_use]
    pub fn module_of(&self, r: ObjectRef) -> Option<&'static str> {
        self.modules
            .iter()
            .find(|(_, m)| **m == r)
            .map(|(name, _)| *name)
    }

    /// 判断值是否为本注册表管理的模块单例。
    #[must_use]
    pub fn is_module_object(&self, val: Value) -> bool {
        matches!(val, Value::Object(r) if self.modules.values().any(|m| *m == r))
    }

    /// 全部模块单例句柄（GC 根源登记用）。
    pub fn module_handles(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        self.modules.values().copied()
    }
}

/// 统一分派入口：`receiver` 是注册表模块单例且 `模块名.方法名` 命中时调用。
///
/// 返回 `None` 表示未命中（调用方走既有路径）；`Some(Ok(v))`/`Some(Err(e))`
/// 表示已处理。
pub fn try_dispatch(
    vm: &mut Vm,
    receiver: Value,
    method: &str,
    args: &[Value],
) -> Option<Result<Value, VmError>> {
    let Value::Object(r) = receiver else {
        return None;
    };
    let key = match &vm.heap[r.index()] {
        // 形态一：GET_PROP 后调用（receiver 是 NativeFn "模块.方法"）。
        // 优先尝试「名.方法」键（构造器静态方法，如 AsyncResource.bind），
        // 未命中回退原名（保持 console.log 等既有分派）。
        HeapObject::NativeFn { name, .. } if name.contains('.') => {
            let with_method = format!("{name}.{method}");
            if vm.builtin_registry.dispatch.contains_key(&with_method) {
                with_method
            } else {
                name.clone()
            }
        }
        // 形态二：模块单例直调（receiver 是模块对象或类构造器）
        HeapObject::Ordinary { proto, .. } => {
            let proto_has = |vm: &Vm, p: ObjectRef, key: &str| -> bool {
                matches!(vm.heap.get(p.index()), Some(HeapObject::Ordinary { .. }))
                    && vm.has_own_slot(p.index(), key)
            };
            let is_ee = vm.has_own_slot(r.index(), "_isEventEmitter")
                || proto.is_some_and(|p| proto_has(vm, p, "_isEventEmitter"));
            let is_stream = vm.has_own_slot(r.index(), "_isStream")
                || proto.is_some_and(|p| proto_has(vm, p, "_isStream"));
            if let Some(ns) = builtin_ns(vm, r) {
                format!("{ns}.{method}")
            } else if vm.has_own_slot(r.index(), "_isBuffer") {
                format!("buffer:instance.{method}")
            } else if is_ee {
                format!("events:instance.{method}")
            } else if is_stream {
                format!("stream.{method}")
            } else if let Some(module_name) = vm.builtin_registry.module_of(r) {
                format!("{module_name}.{method}")
            } else {
                return None;
            }
        }
        HeapObject::NativeCtor { .. } => {
            if let Some(module_name) = vm.builtin_registry.module_of(r) {
                format!("{module_name}.{method}")
            } else {
                return None;
            }
        }
        HeapObject::EventEmitter { .. } => {
            format!("events:instance.{method}")
        }
        HeapObject::Readable { .. } => {
            format!("stream.{method}")
        }
        _ => return None,
    };
    let handler = vm.builtin_registry.lookup(&key)?;
    set_current_receiver(receiver);
    Some(handler(vm, args))
}

/// 供其它文件安全地给单例挂属性（绕过借用拆分）。
pub fn set_module_prop(
    vm: &mut Vm,
    obj: ObjectRef,
    key: &str,
    value: Value,
) -> Result<(), VmError> {
    let _ = vm.set_property(Value::Object(obj), key, value);
    Ok(())
}

/// 模块对象识别辅助（供解释器与测试）。
pub fn is_module_heap_obj(vm: &Vm, r: ObjectRef) -> bool {
    // 只是形状辅助：模块对象都是 Ordinary；不做额外区分
    matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. }))
}
