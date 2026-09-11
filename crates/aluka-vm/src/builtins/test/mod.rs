//! `test` 内置模块（Phase 8）：Node 22 `node:test` 的 describe/it/test/
//! hooks/mock/assert 表面与「注册 + 顺序执行」模型。
//!
//! 模块导出值本身即**可调用的 `test` 函数**（Node 22 实测锚定：
//! `typeof require("node:test") === "function"`、`t.it === t.test === t`、
//! `t.describe === t.suite`、`t.skip === t.test.skip`）；其余导出
//! （before/after/beforeEach/afterEach/skip/todo/only/mock/assert/register/
//! snapshot/run/default）都是挂在该函数**自有属性**上的值。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodetest/`）实际注册的表面：
//! - 注册面：`test`/`it`（同一函数对象）、`describe`/`suite`（同一函数对象）、
//!   二者各自的 `skip`/`todo`/`only` 函数属性形态（M5.4 切片一）、
//!   `beforeEach`/`afterEach`/`before`/`after`、顶层 `skip`/`todo`/`only`
//!   别名、`mock`、`assert`（复用 node:assert 单例）、`register`、
//!   `snapshot`、`run`、`default`（模块自身——CJS 互操作；Node 22.23.1
//!   实测 `t.default` 为 undefined，本项为本仓保留的互操作超集）；
//! - 执行面：`run()` 程序化运行（Node 语义：返回事件流，异步派发
//!   `test:start`/`test:pass`/`test:fail`/`test:skip`/`test:todo`/
//!   `test:plan`/`end`；派发任务经宏任务调度——与 Go `PostTask` 一致，
//!   需要事件循环存活，即脚本存在定时器/微任务时才会驱动）；
//! - 执行面（M5.4 切片一）：`auto_run()` 收尾自动运行——`aluka test`
//!   子命令用（见函数注释：显式 `run()` 过则不重复执行）；
//! - describe 函数体注册时同步执行（Node 语义）。
//!
//! `todo` 语义（Node 22 实测锚定，M5.4 切片一修正）：`t.todo(name)`
//! **无回调**时不执行、报告 `ok` + `# TODO`；**有回调**时执行，失败行状态为
//! `not ok ... # TODO` 但汇总 `fail` 不计入（只计 `todo`）；`skip` 一律不执行。
//!
//! 函数属性形态的等价性（Node 22 实测锚定）：`t.it === t`、`t.describe ===
//! t.suite` 均为 true，且四者的 `skip`/`todo`/`only` 属性都是函数。用例级
//! 形态复用顶层 skip/todo/only 的处理器（`it.skip(n, f)` ≡
//! `it(n, {skip: true}, f)`）；套件级形态为套件标记（`describe.skip(n, f)` ≡
//! `describe(n, {skip: true}, f)`），行为与 `register_describe` 一致，只是把
//! 对应 flag 置真。
//!
//! 已知限制与口径说明（引擎能力边界，逐条对齐 Node/Go 实测行为后记录）：
//! - 纯同步脚本（无任何定时器/微任务）中 Go 丢弃 `run()` 的派发任务
//!   （实测），Rust 侧宏任务无条件驱动——已知偏离；
//! - `spy.mock.calls` 观测面**可达**（旧注释曾称不可达，已按代码更正）：
//!   安装 spy 时经 `set_native_fn_property` 挂 `.mock`（mock.rs:244），每次
//!   调用就地追加调用记录；
//! - 模块导出函数是原生函数对象（"test.test"），`CALL_METHOD` 分派对
//!   NativeFn 接收者先试「接收者原名.方法名」复合键、未命中回退接收者原名，
//!   故 **`node:test` 全方法面与属性形态都必须显式登记复合键**（见 build 末尾
//!   登记表）；未登记的方法名（Node 本身也没有的名字）会被回退吞成
//!   `register_it`——`test.test`/`test.test.*` 之外无其它入口。

pub mod asserts;
pub mod context;
pub mod mock;
pub mod registry;
pub mod runner;
pub mod state;

use crate::builtins::test_reporters::{ReportCase, ReportCounts, ReportStatus, ReporterKind};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// `require("test")` / `require("node:test")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "test",
    build,
};

thread_local! {
    /// `run()` 建立的事件流（宏任务派发时取用）。
    static RUN_STREAM: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// 脚本是否已显式调用过 `run()`（`auto_run` 去重标记；模块 build 时清零）。
    static RUN_EXPLICIT: Cell<bool> = const { Cell::new(false) };
}

/// 是否可调用值（函数）。
pub fn is_function_value(vm: &Vm, v: Value) -> bool {
    matches!(v.case(), ValueCase::Object(r)
    if matches!(
        vm.heap.get(r.index()),
        Some(HeapObject::Closure { .. }) | Some(HeapObject::NativeFn { .. })
    ))
}

/// 事件流属性读取（pipe 返回自身——对齐 Go 最小 stream 语义）。
fn stream_pipe(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(crate::builtins::current_receiver())
}

/// 构建 node:test 模块导出值（对齐 Go `NewTest`）。
///
/// 返回的句柄**就是**那个可调用的 `test` 函数对象（Node 22 实测：模块导出值
/// 本身是函数），其余导出全部挂在它的自有属性上。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    // 注册表重置（对齐 Go：每个测试文件运行前 ResetTestRegistry）。
    registry::reset();
    // 显式 run() 标记清零（与注册表同生命周期：新测试文件不复用旧标记）。
    RUN_EXPLICIT.with(|f| f.set(false));

    // 模块导出对象 = `test` 函数自身（Node 22 实测：`t.test === t.it === t`）；
    // 各导出经 `set_property` 的 NativeFn 自有属性分支挂载（先例见 mock spy
    // 的 `.mock`），属性读取走同一张表，`typeof require("node:test")` 因此为
    // "function"、值调用形态 `test(name, fn)` 直接命中下面登记的 "test.test"。
    let m = vm.alloc_native_fn("test.test");

    // 顶层 shorthand skip/todo/only：Node 22 实测与函数属性形态是**同一函数
    // 对象**（`t.skip === t.test.skip`），故两处挂载点共用同一句柄。
    for (attr, key) in [
        ("skip", "test.skip"),
        ("todo", "test.todo"),
        ("only", "test.only"),
    ] {
        let attr_fn = vm.alloc_native_fn(key);
        vm.set_native_fn_property(m, attr, Value::Object(attr_fn));
    }

    // it/test 指向自身（同上实测：`t.it === t`、`t.test === t`）。
    for prop in ["it", "test"] {
        set_module_prop(vm, m, prop, Value::Object(m))?;
    }

    // describe/suite：Node 22 实测 `t.describe === t.suite`（同一函数对象）。
    // 函数属性形态为**套件级**标记：`describe.skip(n, f)` ≡
    // `describe(n, { skip: true }, f)`（套件整体标 SKIP），处理器见下方
    // register_describe_skip/todo/only。
    let describe_fn = vm.alloc_native_fn("test.describe");
    for (attr, key) in [
        ("skip", "test.describeSkip"),
        ("todo", "test.describeTodo"),
        ("only", "test.describeOnly"),
    ] {
        let attr_fn = vm.alloc_native_fn(key);
        vm.set_native_fn_property(describe_fn, attr, Value::Object(attr_fn));
    }
    for prop in ["describe", "suite"] {
        set_module_prop(vm, m, prop, Value::Object(describe_fn))?;
    }
    // 钩子。
    for prop in ["beforeEach", "afterEach", "before", "after"] {
        let fn_ref = vm.alloc_native_fn(&format!("test.{prop}"));
        set_module_prop(vm, m, prop, Value::Object(fn_ref))?;
    }

    // mock：模块级 MockTracker（不自动还原）。
    let tracker = mock::new_tracker(vm, mock::TrackerScope::Global);
    set_module_prop(vm, m, "mock", tracker)?;

    // assert：node:assert 模块对象（Node 22：test.assert 可用）。
    if let Some(assert_ref) = registry_module_of(vm, "assert") {
        set_module_prop(vm, m, "assert", Value::Object(assert_ref))?;
    }
    // 顶层 shorthand skip/todo/only 已在上方作为 `m` 自身的函数属性挂载
    // （Node 22：`t.skip === t.test.skip`，同一函数对象，不重复分配）。

    // register(name, fn)：注册自定义断言（挂到 t.assert）。
    let register_fn = vm.alloc_native_fn("test.register");
    set_module_prop(vm, m, "register", Value::Object(register_fn))?;

    // snapshot 对象（Node 22：挂在 t.snapshot 下）。
    let snapshot_obj = vm.alloc_ordinary();
    for (prop, name) in [
        (
            "setDefaultSnapshotSerializers",
            "test.snapshot.setDefaultSnapshotSerializers",
        ),
        (
            "setResolveSnapshotPath",
            "test.snapshot.setResolveSnapshotPath",
        ),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, snapshot_obj, prop, Value::Object(fn_ref))?;
    }
    set_module_prop(vm, m, "snapshot", Value::Object(snapshot_obj))?;

    // run(options)：程序化运行（返回事件流；任务经宏任务派发）。
    let run_fn = vm.alloc_native_fn("test.run");
    set_module_prop(vm, m, "run", Value::Object(run_fn))?;

    // default：CJS 互操作（指向模块自身）。
    set_module_prop(vm, m, "default", Value::Object(m))?;

    // --- 分派表登记 ---
    register_handler(registry, "test", "it", register_it);
    register_handler(registry, "test", "test", register_it);
    register_handler(registry, "test", "describe", register_describe);
    register_handler(registry, "test", "suite", register_describe);
    register_handler(registry, "test", "beforeEach", |vm, args| {
        hook_register("beforeEach", vm, args)
    });
    register_handler(registry, "test", "afterEach", |vm, args| {
        hook_register("afterEach", vm, args)
    });
    register_handler(registry, "test", "before", |vm, args| {
        hook_register("before", vm, args)
    });
    register_handler(registry, "test", "after", |vm, args| {
        hook_register("after", vm, args)
    });
    register_handler(registry, "test", "skip", register_skip);
    register_handler(registry, "test", "todo", register_todo);
    register_handler(registry, "test", "only", register_only);
    // 模块对象自身是 NativeFn "test.test"（含点）：`CALL_METHOD` 对 NativeFn
    // 接收者先按「接收者原名.方法名」拼键、未命中即回退到接收者原名（见
    // `builtins::try_dispatch` 形态一），而原名键 "test.test" 就是 `register_it`
    // ——不登记复合键的话 `t.run()`/`t.describe()` 会全部被吞成「注册用例」，
    // 故 `t.<method>()` 的复合键必须逐一登记（`it.skip(...)`/`t.skip(...)`
    // 同样落在这层）。裸调用形态（`const f = it.skip; f(...)`）则经属性值原名
    //（"test.skip"）直接命中上表。处理器本体同一处，不另建语义。
    register_handler(registry, "test.test", "test", register_it);
    register_handler(registry, "test.test", "it", register_it);
    register_handler(registry, "test.test", "describe", register_describe);
    register_handler(registry, "test.test", "suite", register_describe);
    register_handler(registry, "test.test", "beforeEach", |vm, args| {
        hook_register("beforeEach", vm, args)
    });
    register_handler(registry, "test.test", "afterEach", |vm, args| {
        hook_register("afterEach", vm, args)
    });
    register_handler(registry, "test.test", "before", |vm, args| {
        hook_register("before", vm, args)
    });
    register_handler(registry, "test.test", "after", |vm, args| {
        hook_register("after", vm, args)
    });
    register_handler(registry, "test.test", "skip", register_skip);
    register_handler(registry, "test.test", "todo", register_todo);
    register_handler(registry, "test.test", "only", register_only);
    register_handler(registry, "test.test", "register", register_custom);
    register_handler(registry, "test.test", "run", run);
    // 套件级函数属性形态（describe.skip / suite.todo / ...）：套件标记
    // 语义 + 复合键，理由同上。
    register_handler(registry, "test", "describeSkip", register_describe_skip);
    register_handler(registry, "test", "describeTodo", register_describe_todo);
    register_handler(registry, "test", "describeOnly", register_describe_only);
    register_handler(registry, "test.describe", "skip", register_describe_skip);
    register_handler(registry, "test.describe", "todo", register_describe_todo);
    register_handler(registry, "test.describe", "only", register_describe_only);
    register_handler(registry, "test", "register", register_custom);
    register_handler(registry, "test", "run", run);
    register_handler(
        registry,
        "test.snapshot",
        "setDefaultSnapshotSerializers",
        noop,
    );
    register_handler(registry, "test.snapshot", "setResolveSnapshotPath", noop);
    register_handler(registry, "test:postedRun", "task", posted_run);
    register_handler(registry, "test:streamPipe", "pipe", stream_pipe);
    register_handler(registry, "test:stream", "compose", stream_compose);
    register_handler(registry, "test:stream.compose", "forward", compose_forward);
    register_handler(registry, "test:stream.compose", "pipe", compose_pipe);

    context::register_handlers(registry);
    mock::register_handlers(registry);

    Ok(m)
}

/// 读取注册表中其它模块单例（assert 复用）。
fn registry_module_of(vm: &Vm, name: &str) -> Option<ObjectRef> {
    vm.builtin_registry.module(name)
}

/// 无操作处理器（snapshot 配置面——Node 语义占位）。
fn noop(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// it/test 注册（对齐 Go `register`）：(name, fn) / (fn) / (name, opts, fn)。
fn register_it(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (name, fn_val, opts) = parse_options(vm, args);
    if !is_function_value(vm, fn_val) && !opts.skip && !opts.todo {
        return Err(asserts::type_fail(vm, "it() requires a function"));
    }
    let fn_val = if is_function_value(vm, fn_val) {
        fn_val
    } else {
        Value::Undefined
    };
    registry::push_test(registry::TestNode {
        name,
        fn_val,
        skip: opts.skip,
        todo: opts.todo,
        only: opts.only,
        concurrent: opts.concurrency,
    });
    Ok(Value::Undefined)
}

/// describe 注册并同步执行函数体（其内的 it/describe/beforeEach 注册子项）。
fn register_describe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_suite(vm, args, None)
}

/// 套件级 skip/todo/only 变体（函数属性形态 describe.skip/todo/only）：
/// 与 [`register_describe`] 行为完全一致，只是把对应 flag 置真——等价于
/// options 形态 `describe(n, {skip: true}, f)`。
fn register_describe_skip(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_suite(vm, args, Some(Flag::Skip))
}

/// 套件级 todo 变体（`describe.todo(n, f)`）。
fn register_describe_todo(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_suite(vm, args, Some(Flag::Todo))
}

/// 套件级 only 变体（`describe.only(n, f)`；套件体必须传函数）。
fn register_describe_only(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_suite(vm, args, Some(Flag::Only))
}

/// 套件注册共用实现：`push_suite` + （传入函数则）同步执行函数体 +
/// `pop_suite`；`flag` 为函数属性形态带来的套件标记。
fn register_suite(vm: &mut Vm, args: &[Value], flag: Option<Flag>) -> Result<Value, VmError> {
    let (name, fn_val, mut opts) = parse_options(vm, args);
    match flag {
        Some(Flag::Skip) => opts.skip = true,
        Some(Flag::Todo) => opts.todo = true,
        Some(Flag::Only) => opts.only = true,
        None => {}
    }
    if !is_function_value(vm, fn_val) && !opts.skip && !opts.todo {
        return Err(asserts::type_fail(vm, "describe() requires a function"));
    }
    registry::push_suite(registry::SuiteNode {
        name,
        parent: None,
        before_hooks: Vec::new(),
        after_hooks: Vec::new(),
        before_each: Vec::new(),
        after_each: Vec::new(),
        children: Vec::new(),
        suites: Vec::new(),
        tests: Vec::new(),
        skip: opts.skip,
        todo: opts.todo,
        only: opts.only,
        concurrent: opts.concurrency,
    });
    // 同步执行 suite 函数体；注册期错误按 Go `ReportUncaught` 语义吞掉
    // （进程继续，剩余注册与运行不受影响）。
    if is_function_value(vm, fn_val) {
        let _ = vm.invoke_callable(fn_val, Value::Undefined, &[]);
    }
    registry::pop_suite();
    Ok(Value::Undefined)
}

/// 标记形态枚举（skip/todo/only 用例级与套件级变体注册共用）。
#[derive(Clone, Copy)]
enum Flag {
    /// 跳过。
    Skip,
    /// 待办。
    Todo,
    /// 仅运行。
    Only,
}

/// `it.skip`/`test.skip` 处理器（顶层 `skip` 别名与函数属性形态同一处）。
fn register_skip(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_flagged(vm, args, Flag::Skip)
}

/// `it.todo`/`test.todo` 处理器。
fn register_todo(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_flagged(vm, args, Flag::Todo)
}

/// `it.only`/`test.only` 处理器。
fn register_only(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    register_flagged(vm, args, Flag::Only)
}

/// skip/todo/only 变体注册（对齐 Go skipReg/todoReg/onlyReg）。
fn register_flagged(vm: &mut Vm, args: &[Value], flag: Flag) -> Result<Value, VmError> {
    let (name, fn_val, opts) = parse_options(vm, args);
    if matches!(flag, Flag::Only) && !is_function_value(vm, fn_val) {
        return Err(asserts::type_fail(vm, "it.only() requires a function"));
    }
    let fn_val = if is_function_value(vm, fn_val) {
        fn_val
    } else {
        Value::Undefined
    };
    registry::push_test(registry::TestNode {
        name,
        fn_val,
        skip: matches!(flag, Flag::Skip) || opts.skip,
        todo: matches!(flag, Flag::Todo) || opts.todo,
        only: matches!(flag, Flag::Only) || opts.only,
        concurrent: opts.concurrency,
    });
    Ok(Value::Undefined)
}

/// 钩子注册（beforeEach/afterEach/before/after——挂到当前套件）。
fn hook_register(key: &str, vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() || !is_function_value(vm, args[0]) {
        return Err(asserts::type_fail(
            vm,
            &format!("{key}() requires a function"),
        ));
    }
    let hook = args[0];
    registry::with(|reg| {
        let cur = *reg.stack.last().expect("栈底恒为根");
        let suite = &mut reg.suites[cur];
        match key {
            "beforeEach" => suite.before_each.push(hook),
            "afterEach" => suite.after_each.push(hook),
            "before" => suite.before_hooks.push(hook),
            "after" => suite.after_hooks.push(hook),
            _ => {}
        }
    });
    Ok(Value::Undefined)
}

/// `(name, options?, fn)` 形态解析（对齐 Go `parseOptions` + `applyTestOpts`）。
fn parse_options(vm: &mut Vm, args: &[Value]) -> (String, Value, registry::TestOpts) {
    let mut opts = registry::TestOpts::default();
    match args.len() {
        0 => ("anonymous".to_owned(), Value::Undefined, opts),
        1 => {
            let (name, fn_val) = registry::test_name_and_fn(vm, args);
            (name, fn_val, opts)
        }
        2 => {
            let name = vm.format_value(args[0]);
            if is_function_value(vm, args[1]) {
                (name, args[1], opts)
            } else {
                apply_opts(vm, args[1], &mut opts);
                (name, Value::Undefined, opts)
            }
        }
        _ => {
            let name = vm.format_value(args[0]);
            apply_opts(vm, args[1], &mut opts);
            (name, args[2], opts)
        }
    }
}

/// options 对象读取 skip/todo/only。
fn apply_opts(vm: &mut Vm, o: Value, opts: &mut registry::TestOpts) {
    if let Some(r) = o.as_object() {
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })) {
            registry::apply_test_opts(vm, o, opts);
        }
    }
}

/// `register(name, fn)`：注册自定义断言（对齐 Go：fn 校验 + 挂 t.assert）。
fn register_custom(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(asserts::type_fail(
            vm,
            "register(name, fn) requires a name and a function",
        ));
    }
    let name = vm.format_value(args[0]);
    if !is_function_value(vm, args[1]) {
        return Err(asserts::type_fail(
            vm,
            "register(name, fn): fn must be a function",
        ));
    }
    registry::register_custom_assert(&name, args[1]);
    Ok(Value::Undefined)
}

/// 构造测试事件流对象：发射器实例 + `_builtinNs` 分派（on/emit 等复用
/// `events:instance` 处理器，另加 pipe——对齐 Go 最小 stream 语义）。
fn new_test_stream(vm: &mut Vm) -> ObjectRef {
    let stream = crate::builtins::events::create_emitter_instance(vm);
    let ns_val = Value::Object(vm.alloc_string("test:stream".to_owned()));
    let _ = vm.set_property(Value::Object(stream), "_builtinNs", ns_val);
    for m in [
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
        let key = format!("events:instance.{m}");
        if let Some(h) = vm.builtin_registry.lookup(&key) {
            register_handler(&mut vm.builtin_registry, "test:stream", m, h);
        }
    }
    let pipe_fn = vm.alloc_native_fn("test:stream.pipe");
    let _ = vm.set_property(Value::Object(stream), "pipe", Value::Object(pipe_fn));
    let compose_fn = vm.alloc_native_fn("test:stream.compose");
    let _ = vm.set_property(Value::Object(stream), "compose", Value::Object(compose_fn));
    stream
}

// ---------------------------------------------------------------------------
// TestsStream.compose（M5.4）：`run().compose(reporter).pipe(dest)` 管道。
//
// Node 22.23.1 口径：TestsStream extends Readable（object mode，push
// `{type, data}` 分块），`compose` 返回新 Readable——报告器消费事件分块、
// 产出格式化文本，`pipe(dest)` 把文本写入目标流。本实现以挂起表 +
// 事件转发近似：compose 时向源流订阅全部测试事件，每个事件经报告器
// `write({type, data})` 格式化，文本直通 `dest.write`（pipe 晚于事件时先
// 缓冲、pipe 时补冲）。
// ---------------------------------------------------------------------------

thread_local! {
    /// 组合流状态：composed 对象句柄 id → 目的地 / 补冲缓冲。
    static COMPOSED: RefCell<HashMap<u32, ComposedState>> = RefCell::new(HashMap::new());
}

/// 组合流状态。
struct ComposedState {
    /// `pipe(dest)` 的目的地（值可直接 `write`；晚接时先缓冲）
    dest: Option<Value>,
    /// pipe 前已产出、尚未落地的文本
    buffer: Vec<String>,
}

/// `run().compose(reporter)`：报告器可为实例（有 `_reporterKind`）或工厂函数
/// （调用后得实例）。返回组合流（`constructor.name === 'Readable'`、可 `pipe`）。
fn stream_compose(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 先取源流：工厂调用会覆盖 current_receiver
    let source = crate::builtins::current_receiver();
    let reporter = match args.first().copied().map(|v| v.case()) {
        Some(r @ ValueCase::Object(_)) => {
            let already = vm
                .get_property(r, "_reporterKind")
                .ok()
                .is_some_and(|v| vm.is_string_value(v));
            if already {
                r
            } else {
                // 工厂函数：调用后得实例
                let inst = vm.invoke_callable(r, Value::Undefined, &[])?;
                let ok = vm
                    .get_property(inst, "_reporterKind")
                    .ok()
                    .is_some_and(|v| vm.is_string_value(v));
                if !ok {
                    return Err(VmError::Thrown(error_value(
                        vm,
                        "TypeError",
                        "The \"reporter\" argument must be a test reporter Transform",
                    )));
                }
                inst
            }
        }
        _ => {
            return Err(VmError::Thrown(error_value(
                vm,
                "TypeError",
                "The \"reporter\" argument must be a test reporter Transform",
            )));
        }
    };

    let composed = vm.alloc_ordinary();
    let ctor = vm.alloc_native_fn("Readable");
    let _ = vm.set_property(Value::Object(composed), "constructor", Value::Object(ctor));
    let pipe_fn = vm.alloc_native_fn("test:stream.compose.pipe");
    let _ = vm.set_property(Value::Object(composed), "pipe", Value::Object(pipe_fn));
    COMPOSED.with(|g| {
        g.borrow_mut().insert(
            composed.0,
            ComposedState {
                dest: None,
                buffer: Vec::new(),
            },
        );
    });

    // 订阅源流的全部测试事件（source 已在函数入口捕获）。
    for ev in [
        "test:start",
        "test:pass",
        "test:fail",
        "test:skip",
        "test:todo",
        "test:plan",
        "end",
    ] {
        let cb = vm.alloc_native_fn("test:stream.compose.forward");
        vm.set_native_fn_property(cb, "_composed", Value::Number(composed.0 as f64));
        vm.set_native_fn_property(cb, "_reporter", reporter);
        let ev_str = vm.alloc_string(ev.to_owned());
        vm.set_native_fn_property(cb, "_event", Value::Object(ev_str));
        let on_fn = vm.get_property(source, "on")?;
        let ev_val = Value::Object(vm.alloc_string(ev.to_owned()));
        vm.invoke_callable(on_fn, source, &[ev_val, Value::Object(cb)])?;
    }
    Ok(Value::Object(composed))
}

/// 事件转发：把 `{type, data}` 事件分块交报告器格式化，文本直通目的地
/// （未 pipe 时缓冲；写目的地在状态锁外执行，避免 borrow 跨调用）。
fn compose_forward(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ValueCase::Object(callee) = crate::builtins::pending_callee() else {
        return Ok(Value::Undefined);
    };
    let composed_id = match vm.get_native_fn_property(callee, "_composed").map(|v| v.case()) {
        Some(ValueCase::Number(n)) if n >= 0.0 => n as u32,
        _ => return Ok(Value::Undefined),
    };
    let reporter = vm
        .get_native_fn_property(callee, "_reporter")
        .unwrap_or(Value::Undefined);
    let ev = match vm.get_native_fn_property(callee, "_event") {
        Some(v) => vm.format_value(v),
        None => return Ok(Value::Undefined),
    };
    let data = args.first().copied().unwrap_or(Value::Undefined);

    // 构造事件分块 {type, data} 并交报告器 write（返回格式化文本）
    let chunk = vm.alloc_ordinary();
    let ev_val = Value::Object(vm.alloc_string(ev));
    let _ = vm.set_property(Value::Object(chunk), "type", ev_val);
    let _ = vm.set_property(Value::Object(chunk), "data", data);
    let write_fn = vm.get_property(reporter, "write")?;
    let text_val = vm.invoke_callable(write_fn, reporter, &[Value::Object(chunk)])?;
    if !vm.is_string_value(text_val) {
        return Ok(Value::Undefined);
    }
    let text = vm.format_value(text_val);
    if text.is_empty() {
        return Ok(Value::Undefined);
    }
    COMPOSED.with(|g| {
        if let Some(st) = g.borrow_mut().get_mut(&composed_id) {
            if st.dest.is_none() {
                st.buffer.push(text.clone());
            }
        }
    });
    let dest = COMPOSED.with(|g| g.borrow().get(&composed_id).and_then(|s| s.dest));
    if let Some(dest) = dest {
        if let Ok(write) = vm.get_property(dest, "write") {
            let out = Value::Object(vm.alloc_string(text));
            vm.invoke_callable(write, dest, &[out])?;
        }
    }
    Ok(Value::Undefined)
}

/// 组合流 `pipe(dest)`：登记目的地并补冲缓冲；返回 destination（Node 语义）。
fn compose_pipe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let dest = args.first().copied().unwrap_or(Value::Undefined);
    let ValueCase::Object(r) = crate::builtins::current_receiver().case() else {
        return Ok(dest);
    };
    let pending: Vec<String> = COMPOSED.with(|g| {
        let mut map = g.borrow_mut();
        let Some(st) = map.get_mut(&r.0) else {
            return Vec::new();
        };
        st.dest = Some(dest);
        std::mem::take(&mut st.buffer)
    });
    for text in pending {
        if let Ok(write) = vm.get_property(dest, "write") {
            let out = Value::Object(vm.alloc_string(text));
            vm.invoke_callable(write, dest, &[out])?;
        }
    }
    Ok(dest)
}

/// 构造带 name 的错误实例。
fn error_value(vm: &mut Vm, name: &str, message: &str) -> Value {
    let err = vm.alloc_error_instance(message);
    let n = vm.alloc_string(name.to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(n));
    Value::Object(err)
}

/// `run(options)`：程序化运行已注册用例。返回事件流（EventEmitter），
/// 派发任务加入宏任务队列（Go `PostTask` 语义：需要事件循环存活）。
fn run(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    // 显式运行标记：`auto_run`（`aluka test` 收尾自动运行）据此避让——
    // 脚本自己已经跑过一遍注册表，再自动跑一次会重复执行用例并重复打印报告。
    RUN_EXPLICIT.with(|f| f.set(true));
    // 事件流：每次 run() 新建（对齐 Go NewEmitterInstance）。
    let stream = new_test_stream(vm);
    RUN_STREAM.with(|s| *s.borrow_mut() = Some(Value::Object(stream)));

    // 派发任务：宏任务（setImmediate 语义——due 取队尾累计，先于后续定时器）。
    let cb = vm.alloc_native_fn("test:postedRun.task");
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    vm.macro_tasks
        .push_back((id, due, 0, Value::Object(cb), false));

    Ok(Value::Object(stream))
}

/// 派发任务：执行注册表全部用例并向事件流派发事件
/// （对齐 Go `run()` 的 PostTask 闭包：test:start → 状态事件 →
/// test:plan → end；cancelled 派发 test:fail——Go 语义）。
fn posted_run(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Some(stream) = RUN_STREAM.with(|s| s.borrow_mut().take()) else {
        return Ok(Value::Undefined);
    };
    let results = runner::run_registered_tests(vm);

    let (mut passing, mut failing, mut skipped, mut todo, mut cancelled) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    for r in &results {
        let name_val = string_val(vm, &r.name);
        let start_data = ordinary(vm, &[("name", name_val)]);
        emit(vm, &stream, "test:start", Value::Object(start_data))?;
        // 同步用例耗时毫秒级为 0（对齐 Go Milliseconds() 的确定性输出）。
        let type_val = string_val(vm, "test");
        let details = ordinary(
            vm,
            &[("duration_ms", Value::Number(0.0)), ("type", type_val)],
        );
        let name_val = string_val(vm, &r.name);
        let data = ordinary(
            vm,
            &[("name", name_val), ("details", Value::Object(details))],
        );
        if r.cancelled {
            cancelled += 1;
            emit(vm, &stream, "test:fail", Value::Object(data))?;
        } else if r.skipped {
            skipped += 1;
            emit(vm, &stream, "test:skip", Value::Object(data))?;
        } else if r.todo {
            todo += 1;
            emit(vm, &stream, "test:todo", Value::Object(data))?;
        } else if r.passed {
            passing += 1;
            emit(vm, &stream, "test:pass", Value::Object(data))?;
        } else {
            failing += 1;
            if let Some(err) = &r.error {
                let err_val = string_val(vm, err);
                let _ = vm.set_property(Value::Object(details), "error", err_val);
            }
            emit(vm, &stream, "test:fail", Value::Object(data))?;
        }
    }
    let plan_end = ordinary(
        vm,
        &[
            ("count", Value::Number(results.len() as f64)),
            ("passing", Value::Number(f64::from(passing))),
            ("failing", Value::Number(f64::from(failing))),
            ("skipped", Value::Number(f64::from(skipped))),
            ("todo", Value::Number(f64::from(todo))),
            ("cancelled", Value::Number(f64::from(cancelled))),
        ],
    );
    let type_val = string_val(vm, "test");
    let plan = ordinary(vm, &[("type", type_val), ("end", Value::Object(plan_end))]);
    emit(vm, &stream, "test:plan", Value::Object(plan))?;
    emit(vm, &stream, "end", Value::Undefined)?;
    Ok(Value::Undefined)
}

/// 向事件流派发事件（经实例 `emit` 方法）。
fn emit(vm: &mut Vm, stream: &Value, event: &str, data: Value) -> Result<(), VmError> {
    let emit_fn = vm.get_property(*stream, "emit")?;
    let event_val = string_val(vm, event);
    vm.invoke_callable(emit_fn, *stream, &[event_val, data])?;
    Ok(())
}

/// 字符串值分配。
fn string_val(vm: &mut Vm, s: &str) -> Value {
    Value::Object(vm.alloc_string(s.to_owned()))
}

/// 快捷构造普通对象（键值对按序写入）。
fn ordinary(vm: &mut Vm, entries: &[(&str, Value)]) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    for (k, v) in entries {
        let _ = vm.set_property(Value::Object(obj), k, *v);
    }
    obj
}

// ---------------------------------------------------------------------------
// M5.4 切片一：`aluka test` 收尾自动运行
// ---------------------------------------------------------------------------

/// 收尾自动运行已注册用例（`aluka test` 子命令用）：执行注册表全部用例，
/// 按 `kind` 生成报告行追加到 `vm.stdout_records`（CLI 侧统一 `println!`，
/// 与 `run_script` 的 `stdout_records()` 路径一致），返回汇总计数。
///
/// 返回 `None`（且不产生任何输出）的三种情形：
/// 1. 脚本已显式调用过 `test.run()`——显式运行已经消费过注册表，自动运行
///    再跑一次会把用例执行两遍并重复打印报告，故直接避让；
/// 2. 注册表为空——普通脚本（`require("node:test")` 但没注册用例）不得被
///    报告行污染；
/// 3. 注册用例全部被过滤（结果为空）——同 2，不做无意义输出。
///
/// 报告格式为**本仓 CLI 契约**（源自 Go CLI `printTestLine`/汇总格式），
/// 不声称与 `node --test` 输出逐字一致；dot 报告器的逐用例标记沿用 Node
/// dot 报告器的 `.`/`X` 形态（失败清单仍取 CLI 契约）。
pub fn auto_run(vm: &mut Vm, kind: ReporterKind) -> Option<ReportCounts> {
    if RUN_EXPLICIT.with(Cell::get) {
        return None;
    }
    if !registry::has_tests() {
        return None;
    }
    let results = runner::run_registered_tests(vm);
    if results.is_empty() {
        return None;
    }

    let cases: Vec<ReportCase> = results
        .iter()
        .map(|r| ReportCase {
            name: r.full_name.clone(),
            status: if r.passed {
                ReportStatus::Ok
            } else {
                ReportStatus::NotOk
            },
            note: if r.skipped {
                "# SKIP".to_owned()
            } else if r.todo {
                "# TODO".to_owned()
            } else {
                String::new()
            },
            error: r.error.clone(),
        })
        .collect();

    // 计数口径与 `posted_run` 的事件派发一致（cancelled > skipped > todo >
    // passed > fail）：cancelled 的用例 passed 为真但独立计数。
    let mut counts = ReportCounts::default();
    for r in &results {
        if r.cancelled {
            counts.cancelled += 1;
        } else if r.skipped {
            counts.skipped += 1;
        } else if r.todo {
            counts.todo += 1;
        } else if r.passed {
            counts.pass += 1;
        } else {
            counts.fail += 1;
        }
    }

    let lines = crate::builtins::test_reporters::format_report_lines(&cases, kind);
    if matches!(kind, ReporterKind::Dot) {
        // dot：逐用例标记无换行（Node dot 报告器 `..X..` 形态），拼接成单条
        // 记录交给 CLI 打印；spec/tap 则一行一条记录。
        vm.stdout_records.push(lines.concat());
    } else {
        vm.stdout_records.extend(lines);
    }

    let failed: Vec<String> = results
        .iter()
        .filter(|r| !r.passed)
        .map(|r| r.full_name.clone())
        .collect();
    let summary = crate::builtins::test_reporters::format_summary(&counts, &failed, kind);
    if !summary.is_empty() {
        vm.stdout_records.push(summary);
    }
    Some(counts)
}

/// GC 根快照：运行流对象与组合流挂起表（目的地 / 报告器堆值）。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    RUN_STREAM.with(|g| {
        if let Some(v) = g.borrow().as_ref() {
            out.push(*v);
        }
    });
    COMPOSED.with(|g| {
        for st in g.borrow().values() {
            if let Some(d) = &st.dest {
                out.push(*d);
            }
        }
    });
}
