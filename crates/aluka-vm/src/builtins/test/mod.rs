//! `test` 内置模块（Phase 8）：Node 22 `node:test` 的 describe/it/test/
//! hooks/mock/assert 表面与「注册 + 顺序执行」模型。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodetest/`）实际注册的表面：
//! - 注册面：`test`/`it`（同一函数）、`describe`/`suite`、`beforeEach`/
//!   `afterEach`/`before`/`after`、顶层 `skip`/`todo`/`only` 别名、
//!   `mock`、`assert`（复用 node:assert 单例）、`register`、`snapshot`、
//!   `run`、`default`（模块自身——CJS 互操作）；
//! - 执行面：`run()` 程序化运行（Node 语义：返回事件流，异步派发
//!   `test:start`/`test:pass`/`test:fail`/`test:skip`/`test:todo`/
//!   `test:plan`/`end`；派发任务经宏任务调度——与 Go `PostTask` 一致，
//!   需要事件循环存活，即脚本存在定时器/微任务时才会驱动）；
//! - describe 函数体注册时同步执行（Node 语义）。
//!
//! 已知限制（引擎能力边界，逐条对齐 Go 实测行为后记录）：
//! - `test.skip`/`describe.skip` 等函数属性形态不可达（原生函数对象无
//!   属性表）——用等价的 options 形态 `it(name, {skip: true}, fn)` 或
//!   顶层 `skip/todo/only`；
//! - 纯同步脚本（无任何定时器/微任务）中 Go 丢弃 `run()` 的派发任务
//!   （实测），Rust 侧宏任务无条件驱动——已知偏离；
//! - spy 函数的 `spy.mock.calls` 观测面不可达（同函数属性限制）。

pub mod asserts;
pub mod context;
pub mod mock;
pub mod registry;
pub mod runner;
pub mod state;

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;

/// `require("test")` / `require("node:test")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "test",
    build,
};

thread_local! {
    /// `run()` 建立的事件流（宏任务派发时取用）。
    static RUN_STREAM: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// 是否可调用值（函数）。
pub fn is_function_value(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r)
    if matches!(
        vm.heap.get(r.index()),
        Some(HeapObject::Closure { .. }) | Some(HeapObject::NativeFn { .. })
    ))
}

/// 事件流属性读取（pipe 返回自身——对齐 Go 最小 stream 语义）。
fn stream_pipe(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(crate::builtins::current_receiver())
}

/// 构建 node:test 模块导出对象（对齐 Go `NewTest`）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    // 注册表重置（对齐 Go：每个测试文件运行前 ResetTestRegistry）。
    registry::reset();

    let m = vm.alloc_ordinary();

    // it/test：同一处理器的两个导出名（Node 22 语义：同一函数对象别名）。
    for (prop, name) in [("it", "test.it"), ("test", "test.test")] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, m, prop, Value::Object(fn_ref))?;
    }
    // describe/suite：suite 是 describe 的别名（Node 22）。
    for (prop, name) in [("describe", "test.describe"), ("suite", "test.suite")] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, m, prop, Value::Object(fn_ref))?;
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

    // 顶层 shorthand：skip/todo/only（Go：从 test.skip 等拷贝的别名）。
    for prop in ["skip", "todo", "only"] {
        let fn_ref = vm.alloc_native_fn(&format!("test.{prop}"));
        set_module_prop(vm, m, prop, Value::Object(fn_ref))?;
    }

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
    register_handler(registry, "test", "skip", |vm, args| {
        register_flagged(vm, args, Flag::Skip)
    });
    register_handler(registry, "test", "todo", |vm, args| {
        register_flagged(vm, args, Flag::Todo)
    });
    register_handler(registry, "test", "only", |vm, args| {
        register_flagged(vm, args, Flag::Only)
    });
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
    });
    Ok(Value::Undefined)
}

/// describe 注册并同步执行函数体（其内的 it/describe/beforeEach 注册子项）。
fn register_describe(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (name, fn_val, opts) = parse_options(vm, args);
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
    });
    // 同步执行 suite 函数体；注册期错误按 Go `ReportUncaught` 语义吞掉
    // （进程继续，剩余注册与运行不受影响）。
    if is_function_value(vm, fn_val) {
        let _ = vm.invoke_callable(fn_val, Value::Undefined, &[]);
    }
    registry::pop_suite();
    Ok(Value::Undefined)
}

/// 标记形态枚举（skip/todo/only 变体注册共用）。
enum Flag {
    /// 跳过。
    Skip,
    /// 待办。
    Todo,
    /// 仅运行。
    Only,
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
    if let Value::Object(r) = o {
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
    stream
}

/// `run(options)`：程序化运行已注册用例。返回事件流（EventEmitter），
/// 派发任务加入宏任务队列（Go `PostTask` 语义：需要事件循环存活）。
fn run(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
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
