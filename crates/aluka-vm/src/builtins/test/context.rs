//! node:test TestContext（Phase 8）：`t.*` 方法面构造与处理器。
//!
//! 移植 Node.js 22 LTS 标准（`nodetest/test_context.go`）：
//! - `t.assert` 系列（消息格式逐字对齐 Go，含 `aluka: assertion error: `
//!   前缀）、`t.plan` 校验、`t.diagnostic`（`# {msg}` TAP 诊断行）；
//! - `t.test(name, fn)` 子测试经微任务调度（同步父未 await → 取消）；
//! - per-test `t.mock`（测试结束自动还原）与 `t.waitFor` 轮询。

use super::asserts::{
    assert_fail, deep_loose_equal, deep_strict_equal, error_message, loose_equal, regexp_test,
    strict_equal, thrown_msg, type_fail,
};
use super::runner;
pub use super::state::{
    StateMut, attach_subtest, current_flags, current_has_subtests, current_id, current_sub_results,
    current_subtest_ids, join_name, push_sub_result, restore_current_mocks, scoped_current,
    subtest_get, with_current_mut,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

/// 状态访问器整体重导出（runner/mod 经 `context::*` 使用）。
pub use super::state::*;

/// 「expected {expected} but got {actual}」失败消息。
fn fail_expected_but_got(vm: &mut Vm, a: Value, b: Value) -> VmError {
    assert_fail(
        vm,
        &format!(
            "expected {} but got {}",
            vm.format_value(b),
            vm.format_value(a)
        ),
    )
}

/// `t.assert.ok(value)`。
pub fn ctx_assert_ok(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let val = args.first().copied().unwrap_or(Value::Undefined);
    if vm.truthy(val) {
        return Ok(Value::Undefined);
    }
    Err(assert_fail(vm, "expected value to be truthy"))
}

/// `t.assert.strictEqual(actual, expected)`。
pub fn ctx_assert_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() < 2 || !strict_equal(vm, a, b) {
        return Err(fail_expected_but_got(vm, a, b));
    }
    Ok(Value::Undefined)
}

/// `t.assert.equal(actual, expected)`。
pub fn ctx_assert_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() < 2 || !loose_equal(vm, a, b) {
        return Err(fail_expected_but_got(vm, a, b));
    }
    Ok(Value::Undefined)
}

/// `t.assert.deepStrictEqual(actual, expected)`。
pub fn ctx_assert_deep_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() < 2 || !deep_strict_equal(vm, a, b) {
        return Err(fail_expected_but_got(vm, a, b));
    }
    Ok(Value::Undefined)
}

/// `t.assert.deepEqual(actual, expected)`。
pub fn ctx_assert_deep_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() < 2 || !deep_loose_equal(vm, a, b) {
        return Err(fail_expected_but_got(vm, a, b));
    }
    Ok(Value::Undefined)
}

/// `t.assert.notStrictEqual(actual, expected)`。
pub fn ctx_assert_not_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() >= 2 && strict_equal(vm, a, b) {
        return Err(assert_fail(vm, "values should not be strictly equal"));
    }
    Ok(Value::Undefined)
}

/// `t.assert.notEqual(actual, expected)`。
pub fn ctx_assert_not_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() >= 2 && loose_equal(vm, a, b) {
        return Err(assert_fail(vm, "values should not be loosely equal"));
    }
    Ok(Value::Undefined)
}

/// `t.assert.notDeepEqual(actual, expected)`。
pub fn ctx_assert_not_deep_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() >= 2 && deep_loose_equal(vm, a, b) {
        return Err(assert_fail(vm, "values should not be deep equal"));
    }
    Ok(Value::Undefined)
}

/// `t.assert.notDeepStrictEqual(actual, expected)`。
pub fn ctx_assert_not_deep_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let (a, b) = (
        args.first().copied().unwrap_or(Value::Undefined),
        args.get(1).copied().unwrap_or(Value::Undefined),
    );
    if args.len() >= 2 && deep_strict_equal(vm, a, b) {
        return Err(assert_fail(vm, "values should not be deep strict equal"));
    }
    Ok(Value::Undefined)
}

/// `t.assert.ifError(value)`。
pub fn ctx_assert_if_error(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if let Some(v) = args.first() {
        if !matches!(v, Value::Undefined | Value::Null) {
            return Err(assert_fail(vm, "ifError got unwanted exception"));
        }
    }
    Ok(Value::Undefined)
}

/// `t.assert.fail([msg])`。
pub fn ctx_assert_fail(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    let msg = match args.first() {
        Some(v) => vm.format_value(*v),
        None => "assertion failed".to_owned(),
    };
    Err(assert_fail(vm, &msg))
}

/// `t.assert.match(string, regexp)`。
pub fn ctx_assert_match(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.len() < 2 {
        return Err(type_fail(vm, "match: string and regexp required"));
    }
    let target = vm.format_value(args[0]);
    if !regexp_test(vm, args[1], &target) {
        return Err(assert_fail(
            vm,
            &format!("match: {target:?} does not match"),
        ));
    }
    Ok(Value::Undefined)
}

/// `t.assert.doesNotMatch(string, regexp)`。
pub fn ctx_assert_does_not_match(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.len() < 2 {
        return Err(type_fail(vm, "doesNotMatch: string and regexp required"));
    }
    let target = vm.format_value(args[0]);
    if regexp_test(vm, args[1], &target) {
        return Err(assert_fail(
            vm,
            &format!("doesNotMatch: {target:?} should not match"),
        ));
    }
    Ok(Value::Undefined)
}

/// `t.assert.throws(fn)`。
pub fn ctx_assert_throws(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.is_empty() {
        return Err(assert_fail(vm, "throws: function required"));
    }
    if !super::is_function_value(vm, args[0]) {
        return Err(assert_fail(vm, "throws: first argument must be a function"));
    }
    if vm.invoke_callable(args[0], Value::Undefined, &[]).is_err() {
        return Ok(Value::Undefined);
    }
    Err(assert_fail(
        vm,
        "throws: expected exception but none was thrown",
    ))
}

/// 检查 promise 已定值是否为「拒绝」近似：非 undefined 兑现值视为拒绝
/// （引擎 promise 拒绝与兑现同形——Go `AwaitPromise` 错误路径的近似移植）。
fn promise_rejected(vm: &mut Vm, pv: Value) -> Result<bool, VmError> {
    vm.drain_microtasks()?;
    if let Value::Object(r) = pv {
        if let Some(HeapObject::Promise { pending, value, .. }) = vm.heap.get(r.index()) {
            return Ok(!*pending && !matches!(value, Value::Undefined));
        }
    }
    Ok(false)
}

/// `t.assert.rejects(fn|promise)`。
pub fn ctx_assert_rejects(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.is_empty() {
        return Err(type_fail(vm, "rejects: async function/promise required"));
    }
    if super::is_function_value(vm, args[0]) {
        return match vm.invoke_callable(args[0], Value::Undefined, &[]) {
            Err(_) => Ok(Value::Undefined), // 同步抛出也算拒绝
            Ok(pv) => {
                if promise_rejected(vm, pv)? {
                    Ok(Value::Undefined)
                } else {
                    Err(assert_fail(vm, "rejects: promise did not reject"))
                }
            }
        };
    }
    if matches!(args[0], Value::Object(_)) {
        return if promise_rejected(vm, args[0])? {
            Ok(Value::Undefined)
        } else {
            Err(assert_fail(vm, "rejects: promise did not reject"))
        };
    }
    Err(type_fail(vm, "rejects: value is not a promise"))
}

/// `t.assert.doesNotReject(fn|promise)`。
pub fn ctx_assert_does_not_reject(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.is_empty() {
        return Ok(Value::Undefined);
    }
    let pv = if super::is_function_value(vm, args[0]) {
        match vm.invoke_callable(args[0], Value::Undefined, &[]) {
            Err(_) => {
                return Err(assert_fail(vm, "doesNotReject: got unwanted rejection"));
            }
            Ok(pv) => pv,
        }
    } else {
        args[0]
    };
    if promise_rejected(vm, pv)? {
        return Err(assert_fail(vm, "doesNotReject: got unwanted rejection"));
    }
    Ok(Value::Undefined)
}

/// `t.assert.snapshot(value)`（Node 22 experimental；无持久化——仅计数）。
pub fn ctx_assert_snapshot(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let _ = with_current_mut(|st| st.add_assert());
    if args.is_empty() {
        return Err(type_fail(vm, "snapshot: value required"));
    }
    Ok(Value::Undefined)
}

/// `t.diagnostic(msg)`：输出 `# {msg}`（TAP 诊断行，透传 stdout）。
pub fn ctx_diagnostic(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(v) = args.first() {
        let msg = vm.format_value(*v);
        vm.stdout_records.push(format!("# {msg}"));
    }
    Ok(Value::Undefined)
}

/// `t.skip()`：标记跳过并以内部错误中断用例（Node 语义）。
pub fn ctx_skip(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    with_current_mut(|st| st.mark_skip());
    Err(thrown_msg(vm, "test skipped via t.skip()"))
}

/// `t.todo()`：标记待办（执行但失败不计）。
pub fn ctx_todo(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    with_current_mut(|st| st.mark_todo());
    Ok(Value::Undefined)
}

/// `t.plan(n)`：声明期望断言数（用例结束时校验）。
pub fn ctx_plan(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(type_fail(vm, "plan: count required"));
    }
    let n = match args.first() {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => *n as u32,
        _ => {
            return Err(type_fail(vm, "plan: count must be a non-negative integer"));
        }
    };
    with_current_mut(|st| st.set_plan(n));
    Ok(Value::Undefined)
}

/// `t.runOnly()`：接受并忽略（Node 语义占位）。
pub fn ctx_run_only(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `t.test(name, fn)`：子测试（微任务调度；返回 promise 供父 await）。
pub fn ctx_test(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (name, fn_val) = super::registry::test_name_and_fn(vm, args);
    if !super::is_function_value(vm, fn_val) {
        return Err(type_fail(vm, "t.test() requires a function"));
    }
    let parent_full = RUN_STATES
        .with(|m| current_id().and_then(|id| m.borrow().get(&id).map(|st| st.full.clone())))
        .or_else(|| {
            SUBTEST_STATES
                .with(|m| current_id().and_then(|id| m.borrow().get(&id).map(|st| st.full.clone())))
        })
        .unwrap_or_default();
    let (sub_id, promise) = attach_subtest(vm, &name, &join_name(&parent_full, &name), fn_val);
    // 子测试调度到微任务队列：父 await 时（drain 微任务）执行；同步父
    // 结束时该微任务仍挂起 → 子测试取消（对齐 Go `EnqueueMicrotask`）。
    let cb = vm.alloc_native_fn("test:subRun.task");
    vm.microtask_queue.push_back(crate::builtins::Job::Call(
        Value::Object(cb),
        Value::Number(sub_id as f64),
    ));
    Ok(promise)
}

/// 子测试调度任务：`before/beforeEach → 用例 → afterEach/after`（Node 语义）。
pub fn sub_run_task(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let sub_id = match args.first() {
        Some(Value::Number(n)) => *n as u64,
        _ => return Ok(Value::Undefined),
    };
    let cancelled = subtest_get(sub_id, |s| s.cancelled).unwrap_or(false);
    let Some(promise_val) = subtest_get(sub_id, |s| s.promise).flatten() else {
        return Ok(Value::Undefined);
    };
    if cancelled {
        if let Value::Object(p) = promise_val {
            vm.fulfill_promise(p, Value::Undefined)?;
        }
        return Ok(Value::Undefined);
    }
    // 父状态的子测试钩子（before 首子测试前一次，after 末子测试后一次）。
    let parent = subtest_get(sub_id, |s| s.parent).unwrap_or(0);
    let (before_hooks, after_hooks, before_each, after_each, first_sub, last_sub) = RUN_STATES
        .with(|m| {
            m.borrow().get(&parent).map_or(
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false, false),
                |st| {
                    (
                        st.before_hooks.clone(),
                        st.after_hooks.clone(),
                        st.before_each.clone(),
                        st.after_each.clone(),
                        st.subtests.first() == Some(&sub_id),
                        st.subtests.last() == Some(&sub_id),
                    )
                },
            )
        });
    let mut hook_err: Option<VmError> = None;
    if first_sub {
        for h in &before_hooks {
            if let Err(e) = runner::invoke_hook_fn(vm, *h) {
                hook_err = Some(e);
                break;
            }
        }
    }
    if hook_err.is_none() {
        for h in &before_each {
            if let Err(e) = runner::invoke_hook_fn(vm, *h) {
                hook_err = Some(e);
                break;
            }
        }
    }
    if let Some(e) = hook_err {
        let (name, full) =
            subtest_get(sub_id, |s| (s.name.clone(), s.full.clone())).unwrap_or_default();
        push_sub_result(
            parent,
            runner::TestResult {
                name,
                full_name: full,
                passed: false,
                skipped: false,
                todo: false,
                cancelled: false,
                error: Some(format!("subtest hook: {}", error_message(vm, &e))),
            },
        );
        if let Value::Object(p) = promise_val {
            vm.fulfill_promise(p, Value::Undefined)?;
        }
        return Ok(Value::Undefined);
    }
    let mut sr = runner::run_subtest_sync(vm, sub_id);
    for h in &after_each {
        if let Err(e) = runner::invoke_hook_fn(vm, *h) {
            sr.passed = false;
            if sr.error.is_none() {
                sr.error = Some(format!("subtest afterEach: {}", error_message(vm, &e)));
            }
        }
    }
    push_sub_result(parent, sr);
    if last_sub {
        for h in &after_hooks {
            if let Err(e) = runner::invoke_hook_fn(vm, *h) {
                let (name, full) =
                    subtest_get(sub_id, |s| (s.name.clone(), s.full.clone())).unwrap_or_default();
                push_sub_result(
                    parent,
                    runner::TestResult {
                        name,
                        full_name: full,
                        passed: false,
                        skipped: false,
                        todo: false,
                        cancelled: false,
                        error: Some(format!("subtest after: {}", error_message(vm, &e))),
                    },
                );
                break;
            }
        }
    }
    if let Value::Object(p) = promise_val {
        vm.fulfill_promise(p, Value::Undefined)?;
    }
    Ok(Value::Undefined)
}

/// 子测试钩子挂接（t.before/t.after/t.beforeEach/t.afterEach 共用）。
pub fn ctx_hook(key: &str, vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() || !super::is_function_value(vm, args[0]) {
        return Err(type_fail(vm, &format!("t.{key}() requires a function")));
    }
    let id = current_id().unwrap_or(0);
    let hook = args[0];
    RUN_STATES.with(|m| {
        if let Some(st) = m.borrow_mut().get_mut(&id) {
            match key {
                "before" => st.before_hooks.push(hook),
                "after" => st.after_hooks.push(hook),
                "beforeEach" => st.before_each.push(hook),
                "afterEach" => st.after_each.push(hook),
                _ => {}
            }
        }
    });
    Ok(Value::Undefined)
}

/// `t.waitFor(condition[, options])`：轮询条件函数直至成功或超时
/// （Node 22.14，P1 语义；轮询周期 10ms，timeout 默认无限）。
pub fn ctx_wait_for(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() || !super::is_function_value(vm, args[0]) {
        return Err(type_fail(vm, "t.waitFor() requires a condition function"));
    }
    let mut timeout_ms = 0u64;
    if let Some(v) = args.get(1).copied() {
        if let Ok(Value::Number(n)) = vm.get_property(v, "timeout") {
            if n > 0.0 {
                timeout_ms = n as u64;
            }
        }
    }
    let cond = args[0];
    let start = std::time::Instant::now();
    loop {
        if runner::invoke_hook_fn(vm, cond).is_ok() {
            return Ok(Value::Undefined);
        }
        if timeout_ms > 0 && start.elapsed().as_millis() as u64 > timeout_ms {
            return Err(thrown_msg(vm, "operation timed out"));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 注册 t 系列处理器（模块 build 时调用一次）。
pub fn register_handlers(registry: &mut crate::builtins::BuiltinRegistry) {
    use crate::builtins::register_handler;
    register_handler(registry, "test:ctx.assert", "ok", ctx_assert_ok);
    register_handler(
        registry,
        "test:ctx.assert",
        "strictEqual",
        ctx_assert_strict_equal,
    );
    register_handler(registry, "test:ctx.assert", "equal", ctx_assert_equal);
    register_handler(
        registry,
        "test:ctx.assert",
        "deepStrictEqual",
        ctx_assert_deep_strict_equal,
    );
    register_handler(
        registry,
        "test:ctx.assert",
        "deepEqual",
        ctx_assert_deep_equal,
    );
    register_handler(
        registry,
        "test:ctx.assert",
        "notStrictEqual",
        ctx_assert_not_strict_equal,
    );
    register_handler(
        registry,
        "test:ctx.assert",
        "notEqual",
        ctx_assert_not_equal,
    );
    register_handler(
        registry,
        "test:ctx.assert",
        "notDeepEqual",
        ctx_assert_not_deep_equal,
    );
    register_handler(
        registry,
        "test:ctx.assert",
        "notDeepStrictEqual",
        ctx_assert_not_deep_strict_equal,
    );
    register_handler(registry, "test:ctx.assert", "ifError", ctx_assert_if_error);
    register_handler(registry, "test:ctx.assert", "fail", ctx_assert_fail);
    register_handler(registry, "test:ctx.assert", "match", ctx_assert_match);
    register_handler(
        registry,
        "test:ctx.assert",
        "doesNotMatch",
        ctx_assert_does_not_match,
    );
    register_handler(registry, "test:ctx.assert", "throws", ctx_assert_throws);
    register_handler(registry, "test:ctx.assert", "rejects", ctx_assert_rejects);
    register_handler(
        registry,
        "test:ctx.assert",
        "doesNotReject",
        ctx_assert_does_not_reject,
    );
    register_handler(registry, "test:ctx.assert", "snapshot", ctx_assert_snapshot);
    register_handler(registry, "test:ctx", "diagnostic", ctx_diagnostic);
    register_handler(registry, "test:ctx", "skip", ctx_skip);
    register_handler(registry, "test:ctx", "todo", ctx_todo);
    register_handler(registry, "test:ctx", "plan", ctx_plan);
    register_handler(registry, "test:ctx", "runOnly", ctx_run_only);
    register_handler(registry, "test:ctx", "test", ctx_test);
    register_handler(registry, "test:ctx", "before", |vm, args| {
        ctx_hook("before", vm, args)
    });
    register_handler(registry, "test:ctx", "after", |vm, args| {
        ctx_hook("after", vm, args)
    });
    register_handler(registry, "test:ctx", "beforeEach", |vm, args| {
        ctx_hook("beforeEach", vm, args)
    });
    register_handler(registry, "test:ctx", "afterEach", |vm, args| {
        ctx_hook("afterEach", vm, args)
    });
    register_handler(registry, "test:ctx", "waitFor", ctx_wait_for);
    register_handler(registry, "test:subRun", "task", sub_run_task);
}
