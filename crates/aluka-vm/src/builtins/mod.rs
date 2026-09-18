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
/// cluster / fork 子进程的 IPC 传输层（无 JS 可见面，仅 `cluster` 内部使用）。
pub(crate) mod cluster_ipc;
pub mod constants;
pub mod crypto;
pub mod dgram;
pub mod diagnostics_channel;
pub mod dns;
pub mod dns_promises;
pub mod dns_resolver;
pub mod domain;
pub mod events;
pub mod fs;
pub mod fs_promises;
pub mod global;
pub mod http;
pub mod http2;
pub mod https;
pub mod inspector;
pub mod inspector_promises;
pub mod markdown;
pub mod module;
pub mod net;
pub mod os;
pub mod path_node;
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
use crate::value::{Value, ValueCase};
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

/// GC 根快照：原生分派线程局部（current receiver / pending callee）——
/// handler 执行期间跨分配存活，漏登记 = 高频回收下 receiver 悬垂
/// （M6.1 压力模式定位：stress ≤1024 才显形，生产阈值下为潜伏缺陷）。
pub(crate) fn dispatch_tls_roots(out: &mut crate::gc::GcRoots) {
    CURRENT_RECEIVER.with(|r| out.push(*r.borrow()));
    PENDING_CALLEE.with(|c| out.push(*c.borrow()));
}

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
    match v.case() {
        ValueCase::Object(s) => match vm.heap.get(s.index()) {
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

thread_local! {
    /// 当前被调用的 NativeFn 函数对象（普通函数调用时 this 为 undefined，
    /// handler 需要函数对象本体——BoundFunction 的目标/属性存储于此）
    static PENDING_CALLEE: std::cell::RefCell<Value> = const { std::cell::RefCell::new(Value::Undefined) };
}

/// 记录当前被调用的 NativeFn 函数对象。
pub fn set_pending_callee(v: Value) {
    PENDING_CALLEE.with(|c| *c.borrow_mut() = v);
}

/// 读取当前被调用的 NativeFn 函数对象。
pub fn pending_callee() -> Value {
    PENDING_CALLEE.with(|c| *c.borrow())
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
            crate::builtins::global::MODULE,
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
    // M6.1 根审计：build 途中的分配可能触发回收，而模块对象/原生函数在
    // build 返回登记前不在任何根集合里——加载窗口内挂起回收（计数继续
    // 累积，装配完成后的首次分配即补上），与 JIT 帧内跳过回收同一不变量。
    vm.gc_suspend();
    for def in defs {
        let module_ref = (def.build)(vm, &mut registry)?;
        registry.modules.insert(def.name, module_ref);
    }
    // 原型方法面（属性挂载 + Function.prototype.toString handler）
    crate::builtins::surface::register_surface(vm, &mut registry);
    // 平台 `path` 模块的方法表：按目标平台转挂 `path/posix` 或 `path/win32`
    // 的实现（此前自带一套 `std::path` 轻量实现，既不折叠 `.`/`..` 也不做
    // 平台化的卷处理——`path.join('a/b','../c')` 输出 `a\b\..\c`）。
    // `path.posix`/`path.win32` 两个子模块恒可用（与平台无关），
    // `path.sep`/`path.delimiter` 随之绑定。
    let (path_methods, path_sep, path_delim) = if cfg!(windows) {
        (
            crate::builtins::path_win32::METHODS,
            crate::builtins::path_win32::SEP,
            crate::builtins::path_win32::DELIMITER,
        )
    } else {
        (
            crate::builtins::path_posix::METHODS,
            crate::builtins::path_posix::SEP,
            crate::builtins::path_posix::DELIMITER,
        )
    };
    for (m, handler) in path_methods {
        register_handler(&mut registry, "path", m, *handler);
    }
    materialize_handler_properties(vm, &registry);

    // 主 `path` 对象是解释器**预建**的（不在 registry.modules），物化遍
    // 覆盖不到——平台方法表新增的方法（parse/format）在此补挂属性。
    if let Some(path_mod) = vm.path_module {
        for (m, _) in path_methods {
            let exists = vm
                .get_property(Value::Object(path_mod), m)
                .is_ok_and(|v| v != Value::Undefined);
            if exists {
                continue;
            }
            let f = vm.alloc_native_fn(&format!("path.{m}"));
            let _ = vm.set_property(Value::Object(path_mod), m, Value::Object(f));
        }
    }

    // 全局裸名（queueMicrotask / structuredClone）的间接调用分派——
    // 直接调用走 Op::Call 硬编码链，取值后调用经分派表（与定时器裸名同规）
    registry
        .dispatch
        .insert("queueMicrotask".to_owned(), global_queue_microtask);
    registry
        .dispatch
        .insert("structuredClone".to_owned(), global_structured_clone);
    // `path` 对象面：`posix`/`win32` 子模块对象 + sep/delimiter 常量
    if let Some(path_mod) = vm.path_module {
        for (sub, methods, sep, delim) in [
            (
                "posix",
                crate::builtins::path_posix::METHODS,
                crate::builtins::path_posix::SEP,
                crate::builtins::path_posix::DELIMITER,
            ),
            (
                "win32",
                crate::builtins::path_win32::METHODS,
                crate::builtins::path_win32::SEP,
                crate::builtins::path_win32::DELIMITER,
            ),
        ] {
            let sub_obj = vm.alloc_ordinary();
            // 子对象方法的分派键必须与方法值（NativeFn 名）**严格同形**：
            // 引擎有两条按 NativeFn 名查表的分派链——`CALL_METHOD` 的普通
            // 对象回退（`interpreter.rs` 的 `lookup(name)`）与
            // `invoke_callable`（`call.rs` 的 `lookup(name)`）。此前只登记了
            // 平台模块的键 `path.<m>`，子对象方法名 `path.posix.<m>` 无人
            // 登记，致 `path.posix.join(...)` 与「提取方法值后调用」两种形态
            // 均抛「[function Function] is not a function」（Node 正常返回）。
            // 不能复用 `path/posix` 的键：那是独立子模块 `require("path/posix")`
            // 的命名空间，与 `path.posix` 子对象不是同一标识。
            let sub_ns = format!("path.{sub}");
            for (m, handler) in methods {
                let f = vm.alloc_native_fn(&format!("{sub_ns}.{m}"));
                let _ = vm.set_property(Value::Object(sub_obj), m, Value::Object(f));
                register_handler(&mut registry, &sub_ns, m, *handler);
            }
            let sep_v = Value::Object(vm.alloc_string(sep.to_owned()));
            let delim_v = Value::Object(vm.alloc_string(delim.to_owned()));
            let _ = vm.set_property(Value::Object(sub_obj), "sep", sep_v);
            let _ = vm.set_property(Value::Object(sub_obj), "delimiter", delim_v);
            let _ = vm.set_property(Value::Object(path_mod), sub, Value::Object(sub_obj));
        }
        let sep_v = Value::Object(vm.alloc_string(path_sep.to_owned()));
        let delim_v = Value::Object(vm.alloc_string(path_delim.to_owned()));
        let _ = vm.set_property(Value::Object(path_mod), "sep", sep_v);
        let _ = vm.set_property(Value::Object(path_mod), "delimiter", delim_v);
    }
    // process.stdout/stderr.write：readline 等把提示与输出写到流对象
    register_handler(
        &mut registry,
        "process.stdout",
        "write",
        stream_write_stdout,
    );
    register_handler(
        &mut registry,
        "process.stderr",
        "write",
        stream_write_stderr,
    );
    vm.builtin_registry = registry;
    if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
        let st = vm
            .builtin_registry
            .module("stream/promises")
            .map(|r| {
                vm.get_property(Value::Object(r), "finished")
                    .is_ok_and(|v| v.is_object())
            })
            .unwrap_or(false);
        eprintln!("[bisect] after register_all finished-fn={st}");
    }
    // 装配完成：恢复回收（累积的分配计数在后续分配点触发补收）
    vm.gc_resume();
    Ok(())
}

/// `process.stdout.write(text[, enc])`：原样直写标准输出（无自动换行——
/// stdout_records 是行模型收尾统一补换行，流写入需逐字输出；readline
/// 提示与后续 console.log 同行的场景依赖此直写）。
fn stream_write_stdout(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    print!("{text}");
    Ok(Value::Boolean(true))
}

/// `process.stderr.write(text[, enc])`：写到标准错误（不经对拍输出流）。
fn stream_write_stderr(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    eprint!("{text}");
    Ok(Value::Boolean(true))
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

/// 把分派表里的每一项补成模块对象的**自有属性**。
///
/// Node 语义：`fs.readFileSync` 是可取值存槽、可 `typeof`、可被 ESM 命名
/// 导入、可展开的一等属性。此前只有各模块 `build` 里硬编码的一小部分名字
/// 挂了属性，其余仅能经**直接成员调用**（`fs.readFileSync(p)`）由解释器
/// 拦截分派——`import { readFileSync } from "node:fs"` 与
/// `const { existsSync } = fs` 一律拿到 undefined。
///
/// 此处按注册表统一补齐：已存在的属性（含常量与手工挂载项）保持原值，
/// 缺失者挂同名 `NativeFn` 占位——调用经解释器的原生函数分派回到同一
/// handler，属性读取/判存/展开则与 Node 一致。
fn materialize_handler_properties(vm: &mut Vm, registry: &BuiltinRegistry) {
    for (module_name, module_obj) in &registry.modules {
        // 只处理**公开模块名**（Node `builtinModules` 清单）：注册表里还挂着
        // 内部槽位——`events:instance`（实例原型面）、`fs.stat`、`moduleLoader`
        // 等。往那些槽位补属性会污染实例原型链（`for (const k in server)`
        // 突然枚举出 15 个 EventEmitter 方法），故按清单白名单收口。
        if !crate::builtins::module::is_public_module_name(module_name) {
            continue;
        }
        let prefix = format!("{module_name}.");
        for key in registry.dispatch.keys() {
            let Some(method) = key.strip_prefix(&prefix) else {
                continue;
            };
            // 只补模块直系方法（`fs.stat.isFile` 这类复合键留给各自槽位）
            if method.is_empty() || method.contains('.') {
                continue;
            }
            if vm
                .get_property(Value::Object(*module_obj), method)
                .is_ok_and(|existing| existing != Value::Undefined)
            {
                continue;
            }
            if std::env::var("ALUKA_MAT_DEBUG").is_ok() {
                eprintln!("[mat] {module_name}.{method} <- {key}");
            }
            let fn_ref = vm.alloc_native_fn(key);
            let _ = vm.set_property(Value::Object(*module_obj), method, Value::Object(fn_ref));
        }
    }
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
        matches!(val.case(), ValueCase::Object(r) if self.modules.values().any(|m| *m == r))
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
    let ValueCase::Object(r) = receiver.case() else {
        return None;
    };
    // 实例**自有同名键**优先于内置原型分派：内置实例方法（Date/Map/Set 等）
    // 在原型上以 NativeFn 存在，用户覆写实例属性后须走覆写值
    //（`d.toString = f` 后 `d.toString()` 调 f；S15.9.5 族）。
    // 仅对 Ordinary 实例生效（原生堆变体无自有属性面）。
    // 用户**闭包**覆写实例方法时让位内置分派（`d.toString = function(){...}`
    // 后调用户函数；S15.9.5 族）。
    // 判定收窄到 Closure：内建对象以 NativeFn 挂同名成员极其普遍
    // （`Reflect.apply`、fs.Stats.isFile、Proxy 的 trap 名等），一律让位会
    // 破坏这些形态（实测 gen-builtin-fs / m1-proxy 回归）。
    // 实例自有键与**原型同名方法不同值**时让位内置分派（用户覆写：
    // `d.toString = f` / `Object.defineProperty(d, "toString", ...)`；
    // S15.9.5 族）。仅比较"自有值 ≠ 原型槽值"——内建对象（Reflect.apply、
    // fs.Stats.isFile 等）的自有 NativeFn **就是**分派目标，比较相等故不误拦。
    if vm.proxy_parts(r).is_none()
        && matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. }))
        && let Some(own) = vm.own_value(r.index(), method)
    {
        // 仅当**原型链上存在同名方法**时才比较：原型无同名键者
        // （Reflect 的 apply、fs.Stats 的 isFile 等）其自有成员即分派目标
        let proto_slot = match vm.heap.get(r.index()) {
            Some(HeapObject::Ordinary { proto: Some(p), .. }) => vm.own_value(p.index(), method),
            _ => None,
        };
        if proto_slot.is_some() && proto_slot != Some(own) {
            return None;
        }
    }
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
        // 构造器静态面：模块名键（`Reflect.apply` 等）优先；未登记模块的
        // 全局构造器（Number/Boolean/String/Array 等）回退 `Object.prototype`
        // 的通用方法（`Number.hasOwnProperty("MAX_VALUE")`、
        // `Boolean.hasOwnProperty("prototype")`——S15.7.3/S15.6.3 族）
        HeapObject::NativeCtor { .. } => {
            if let Some(module_name) = vm.builtin_registry.module_of(r) {
                format!("{module_name}.{method}")
            } else if vm
                .builtin_registry
                .dispatch
                .contains_key(&format!("Object.{method}"))
            {
                // Object 自有的静态方法（`Object.hasOwn` / `Object.keys` 等）
                // 优先于原型回退——否则 `Object.hasOwn(o,k)` 会被误派到
                // `Object.prototype.hasOwnProperty`（同前缀），返回错误结果
                format!("Object.{method}")
            } else if vm
                .builtin_registry
                .dispatch
                .contains_key(&format!("Object.prototype.{method}"))
            {
                format!("Object.prototype.{method}")
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
        // 字符串原始值（堆字符串）：按 String.prototype 分派
        //（`"abc".toString()` / `s.valueOf()` / `s.charAt(0)` 等——
        // 此前落 `return None` 致 CALL_METHOD 报 "is not a function"）
        HeapObject::String(_) => format!("String.prototype.{method}"),
        _ => return None,
    };
    let handler = vm.builtin_registry.lookup(&key)?;
    set_current_receiver(receiver);
    set_pending_native_name(&key);

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

/// `queueMicrotask(cb)`：微任务入队（与 Op::Call 硬编码链同源）。
fn global_queue_microtask(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let cb = args.first().copied().unwrap_or(Value::Undefined);
    vm.microtask_queue
        .push_back(crate::builtins::Job::Call(cb, Value::Undefined));
    Ok(Value::Undefined)
}

/// `structuredClone(value[, options])`：结构化克隆（与 worker postMessage 同源）。
fn global_structured_clone(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    vm.structured_clone(args)
}
