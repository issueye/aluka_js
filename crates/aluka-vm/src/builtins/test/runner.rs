//! node:test 执行器（Phase 8）：suite/用例调度、skip/only 过滤、hook 顺序、
//! 子测试统计与结果收集。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodetest/test_runner.go`）：按注册顺序执行
//! children（tests 与 suites 混合——Node 语义）、套件级 before/after、
//! `beforeEach`（外→内）与 `afterEach`（内→外）、only 传播、skip 套件
//! 整体标 SKIP、before 钩子失败全组标失败、子测试独立计数。

use super::asserts::error_message;
use super::context;
use super::registry::{self, Child, Registry};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

/// 单个用例的执行结果（对齐 Go `TestResult`）。
#[derive(Clone, Debug)]
pub struct TestResult {
    /// 用例名。
    pub name: String,
    /// 完整名（"suite > case"）。
    pub full_name: String,
    /// 通过。
    pub passed: bool,
    /// 跳过（`# SKIP`）。
    pub skipped: bool,
    /// 待办（`# TODO`；失败不计）。
    pub todo: bool,
    /// 被取消（父未 await 的子测试——Node 语义，独立统计）。
    pub cancelled: bool,
    /// 失败消息。
    pub error: Option<String>,
}

/// 执行注册表中全部用例，返回结果列表（对齐 Go `RunRegisteredTests`）。
pub fn run_registered_tests(vm: &mut Vm) -> Vec<TestResult> {
    let Some(reg) = registry::snapshot() else {
        return Vec::new();
    };
    let mut results: Vec<TestResult> = Vec::new();
    // only 模式仅在 --test-only 标志下生效（Go：模块化运行恒为 false）。
    run_suite(vm, &reg, 0, "", &mut results, false, false, false);
    results
}

/// 按注册顺序执行套件（children 混合遍历——Node 语义），处理套件级
/// before/after 钩子与 skip/only 传播。
#[allow(clippy::too_many_arguments)]
fn run_suite(
    vm: &mut Vm,
    reg: &Registry,
    suite_idx: usize,
    prefix: &str,
    results: &mut Vec<TestResult>,
    inherited_skip: bool,
    inherited_todo: bool,
    only: bool,
) {
    let suite = &reg.suites[suite_idx];
    let skip = inherited_skip || suite.skip;
    let todo = inherited_todo || suite.todo;
    let only = only || suite.only;
    let pfx = join_name(prefix, &suite.name);

    // 无任何可运行测试：空套件无输出；有测试但不可运行 → 全部标 SKIP；
    // only 模式下非 only 内容完全隐藏（Node 语义）。
    if !suite_has_runnable(reg, suite_idx, skip, only) {
        if has_any_child(reg, suite_idx) && !only {
            mark_all_skipped(reg, suite_idx, &pfx, results);
        }
        return;
    }

    // before（套件级，首用例前执行一次）。
    for &h in &suite.before_hooks {
        if let Err(e) = invoke_hook_fn(vm, h) {
            let msg = format!("before: {}", error_message(vm, &e));
            fail_all_tests(reg, suite_idx, &pfx, results, &msg);
            return;
        }
    }

    // 注册顺序执行 children（tests 与 suites 混合）。
    let children: Vec<Child> = suite.children.clone();
    for child in children {
        match child {
            Child::Suite(sub) => {
                run_suite(vm, reg, sub, &pfx, results, skip, todo, only);
            }
            Child::Test(t) => {
                let name = reg.tests[t].name.clone();
                let full = join_name(&pfx, &name);
                if let Some(mut rs) = run_test_case(vm, reg, suite_idx, t, &full, skip, todo, only)
                {
                    results.append(&mut rs);
                }
            }
        }
    }

    // after（套件级，末用例后执行一次）。
    for &h in &suite.after_hooks {
        if let Err(e) = invoke_hook_fn(vm, h) {
            results.push(TestResult {
                name: suite.name.clone(),
                full_name: join_name(&pfx, "after hook"),
                passed: false,
                skipped: false,
                todo: false,
                cancelled: false,
                error: Some(format!("after: {}", error_message(vm, &e))),
            });
            return;
        }
    }
}

/// 判断套件内是否存在将实际执行的用例（不受 skip 传播与 only 过滤影响）。
fn suite_has_runnable(reg: &Registry, suite_idx: usize, skip: bool, only: bool) -> bool {
    if skip {
        return false;
    }
    let suite = &reg.suites[suite_idx];
    for &t in &suite.tests {
        if reg.tests[t].skip {
            continue;
        }
        if !only || reg.tests[t].only {
            return true;
        }
    }
    for &sub in &suite.suites {
        if suite_has_runnable(reg, sub, false, only) {
            return true;
        }
    }
    false
}

/// 是否有注册内容（区分空套件）。
fn has_any_child(reg: &Registry, suite_idx: usize) -> bool {
    !reg.suites[suite_idx].children.is_empty()
}

/// 套件内全部用例标记 SKIP（递归，保留名称层级）。
fn mark_all_skipped(reg: &Registry, suite_idx: usize, pfx: &str, results: &mut Vec<TestResult>) {
    for child in &reg.suites[suite_idx].children {
        match *child {
            Child::Suite(sub) => {
                mark_all_skipped(reg, sub, &join_name(pfx, &reg.suites[sub].name), results);
            }
            Child::Test(t) => {
                results.push(TestResult {
                    name: reg.tests[t].name.clone(),
                    full_name: join_name(pfx, &reg.tests[t].name),
                    passed: true,
                    skipped: true,
                    todo: false,
                    cancelled: false,
                    error: None,
                });
            }
        }
    }
}

/// 钩子失败时套件内全部用例标失败（Node 语义：before 失败 → 套件失败）。
fn fail_all_tests(
    reg: &Registry,
    suite_idx: usize,
    pfx: &str,
    results: &mut Vec<TestResult>,
    msg: &str,
) {
    for child in &reg.suites[suite_idx].children {
        match *child {
            Child::Suite(sub) => {
                fail_all_tests(
                    reg,
                    sub,
                    &join_name(pfx, &reg.suites[sub].name),
                    results,
                    msg,
                );
            }
            Child::Test(t) => {
                results.push(TestResult {
                    name: reg.tests[t].name.clone(),
                    full_name: join_name(pfx, &reg.tests[t].name),
                    passed: false,
                    skipped: false,
                    todo: false,
                    cancelled: false,
                    error: Some(msg.to_owned()),
                });
            }
        }
    }
}

/// 完整名拼接（"parent > child"，对齐 Go `joinName`）。
fn join_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else if name.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix} > {name}")
    }
}

/// 执行单个用例：beforeEach（外→内）→ 用例 → afterEach（内→外）。
/// 返回 `None` 表示被 only 模式排除（不执行、不输出——Node 语义）；
/// `Some` 首元素为用例自身，其余为子测试（独立计数——Node 统计语义）。
#[allow(clippy::too_many_arguments)]
fn run_test_case(
    vm: &mut Vm,
    reg: &Registry,
    suite_idx: usize,
    test_idx: usize,
    full: &str,
    suite_skip: bool,
    suite_todo: bool,
    only: bool,
) -> Option<Vec<TestResult>> {
    let tc = reg.tests[test_idx].clone();
    // only 模式排除：不执行、不输出。
    if only && !tc.only {
        return None;
    }
    // skip 判定：套件 skip || 用例 skip（显示 # SKIP）。
    if suite_skip || tc.skip {
        return Some(vec![TestResult {
            name: tc.name.clone(),
            full_name: full.to_owned(),
            passed: true,
            skipped: true,
            todo: false,
            cancelled: false,
            error: None,
        }]);
    }
    // todo 判定：套件 todo 传播 || 用例 todo（todo 仍执行，失败不计）。
    let is_todo = tc.todo || suite_todo;

    let mut res = TestResult {
        name: tc.name.clone(),
        full_name: full.to_owned(),
        passed: true,
        skipped: false,
        todo: is_todo,
        cancelled: false,
        error: None,
    };

    // 收集套件链（根 → 叶）。
    let mut chain: Vec<usize> = Vec::new();
    let mut cur = Some(suite_idx);
    while let Some(idx) = cur {
        chain.push(idx);
        cur = reg.suites[idx].parent;
    }
    chain.reverse();

    // beforeEach（外层 → 内层）。
    for &s in &chain {
        let hooks: Vec<Value> = reg.suites[s].before_each.clone();
        for h in hooks {
            if let Err(e) = invoke_hook_fn(vm, h) {
                res.passed = false;
                res.error = Some(format!("beforeEach: {}", error_message(vm, &e)));
                return Some(vec![res]);
            }
        }
    }

    // 用例本体（t.plan 校验 + 子测试），全部在「当前状态」作用域内读取。
    let state_id = context::new_run_state(&tc.name, full, tc.fn_val);
    let snapshot: InvokeSnapshot = context::scoped_current(state_id, || {
        let outcome = invoke_with_state(vm, tc.fn_val);
        // t.mock 的 spy 在测试结束时自动还原（Node 语义）。
        context::restore_current_mocks(vm);
        match outcome {
            Err(e) => InvokeSnapshot::Error(e),
            Ok(InvokeOutcome::SubtestsCancelled) => {
                InvokeSnapshot::Cancelled(context::current_subtest_ids())
            }
            Ok(InvokeOutcome::Done) => {
                InvokeSnapshot::Done(context::plan_error(vm), context::current_sub_results())
            }
        }
    });
    context::drop_state(state_id);
    let (mut invoke_err, plan_err, sub_results, subtest_ids, subs_cancelled) = match snapshot {
        InvokeSnapshot::Error(e) => (Some(e), None, Vec::new(), Vec::new(), false),
        InvokeSnapshot::Cancelled(ids) => {
            let err = VmError::Thrown(Value::Object(
                vm.alloc_string("1 subtest failed".to_owned()),
            ));
            (Some(err), None, Vec::new(), ids, true)
        }
        InvokeSnapshot::Done(pe, sub_results) => (None, pe, sub_results, Vec::new(), false),
    };
    if let Some(e) = invoke_err.take() {
        if is_skip_error(vm, &e) {
            res.skipped = true;
            return Some(vec![res]);
        }
        res.passed = false;
        res.error = Some(error_message(vm, &e));
    } else if let Some(pe) = plan_err {
        res.passed = false;
        res.error = Some(error_message(vm, &pe));
    }
    // 子测试失败传播（Node 语义：'1 subtest failed'）。
    if res.passed && !res.skipped {
        for sr in &sub_results {
            if !sr.passed {
                res.passed = false;
                if res.error.is_none() {
                    let err = sr.error.clone().unwrap_or_default();
                    res.error = Some(format!("{}: {err}", sr.full_name));
                }
            }
        }
    }

    // afterEach（内层 → 外层）。
    for &s in chain.iter().rev() {
        let hooks: Vec<Value> = reg.suites[s].after_each.clone();
        for h in hooks {
            if let Err(e) = invoke_hook_fn(vm, h) {
                res.passed = false;
                res.error = Some(format!("afterEach: {}", error_message(vm, &e)));
                return Some(vec![res]);
            }
        }
    }

    // 子测试独立计数（Node 统计语义）；同步父测试取消的子测试标 cancelled
    // （Passed=true + Cancelled——对齐 Go）。
    let mut out = vec![res];
    if subs_cancelled {
        for id in subtest_ids {
            let (name, sub_full) =
                context::subtest_get(id, |s| (s.name.clone(), s.full.clone())).unwrap_or_default();
            out.push(TestResult {
                name,
                full_name: sub_full,
                passed: true,
                skipped: false,
                todo: false,
                cancelled: true,
                error: None,
            });
        }
    } else {
        out.extend(sub_results);
    }
    Some(out)
}

/// 用例函数调用结果快照（「当前状态」作用域内采集，出作用域后消费）。
enum InvokeSnapshot {
    /// 调用出错（含 t.skip 中断）。
    Error(VmError),
    /// 同步父测试未 await 子测试 → 子测试取消（携带子测试 id 表）。
    Cancelled(Vec<u64>),
    /// 正常结束（plan 校验结果 + 子测试结果）。
    Done(Option<VmError>, Vec<TestResult>),
}

/// 用例函数调用结果。
enum InvokeOutcome {
    /// 正常结束（含 async 完成）。
    Done,
    /// 同步父测试未 await 子测试 → 子测试取消（Node 语义）。
    SubtestsCancelled,
}

/// 用例函数调用（t 参数 + async promise 驱动 + 同步子测试取消判定）。
fn invoke_with_state(vm: &mut Vm, fn_val: Value) -> Result<InvokeOutcome, VmError> {
    let t = context::new_test_context(vm);
    let result = vm.invoke_callable(fn_val, Value::Undefined, &[t])?;
    if is_promise(vm, result) {
        // 父测试 async：drain 微任务驱动 await（子测试经微任务执行）；
        // 兑现非 undefined 值按拒绝近似处理（引擎 promise 拒绝同形）。
        vm.drain_microtasks()?;
        if promise_rejected(vm, result) {
            let msg = rejection_message(vm, result);
            return Err(VmError::Thrown(Value::Object(vm.alloc_string(msg))));
        }
        return Ok(InvokeOutcome::Done);
    }
    // 同步父测试 + 子测试 → 子测试取消、父失败（Node 22 实测语义）。
    if context::current_has_subtests() {
        let ids = context::current_subtest_ids();
        context::cancel_subtests(&ids);
        return Ok(InvokeOutcome::SubtestsCancelled);
    }
    Ok(InvokeOutcome::Done)
}

/// 是否 Promise 值。
fn is_promise(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Promise { .. })))
}

/// 已定 promise 是否为拒绝近似（兑现非 undefined 值）。
fn promise_rejected(vm: &Vm, pv: Value) -> bool {
    if let Value::Object(r) = pv {
        if let Some(HeapObject::Promise { pending, value, .. }) = vm.heap.get(r.index()) {
            return !*pending && !matches!(value, Value::Undefined);
        }
    }
    false
}

/// 从已定 promise 提取拒绝消息。
fn rejection_message(vm: &mut Vm, pv: Value) -> String {
    if let Value::Object(r) = pv {
        if let Some(HeapObject::Promise { value, .. }) = vm.heap.get(r.index()) {
            let v = *value;
            return error_message(vm, &VmError::Thrown(v));
        }
    }
    String::new()
}

/// 是否 t.skip() 的内部中断错误。
fn is_skip_error(vm: &mut Vm, e: &VmError) -> bool {
    error_message(vm, e) == "test skipped via t.skip()"
}

/// 执行钩子函数（before/after/beforeEach/afterEach/条件函数）：独立状态
/// 与 t 上下文（Node 语义）；promise 结果经微任务同步等待。
pub fn invoke_hook_fn(vm: &mut Vm, fn_val: Value) -> Result<(), VmError> {
    let id = context::new_run_state("", "", fn_val);
    let result = context::scoped_current(id, || {
        let t = context::new_test_context(vm);
        vm.invoke_callable(fn_val, Value::Undefined, &[t])
    });
    context::drop_state(id);
    let result = result?;
    if is_promise(vm, result) {
        vm.drain_microtasks()?;
    }
    Ok(())
}

/// 执行子测试（同步）：skip/todo/plan/嵌套子测试语义（对齐 Go
/// `runSubTestSync`）。
pub fn run_subtest_sync(vm: &mut Vm, sub_id: u64) -> TestResult {
    let (name, full, fn_val) =
        context::subtest_get(sub_id, |s| (s.name.clone(), s.full.clone(), s.fn_val)).unwrap_or((
            String::new(),
            String::new(),
            Value::Undefined,
        ));
    let mut res = TestResult {
        name,
        full_name: full,
        passed: true,
        skipped: false,
        todo: false,
        cancelled: false,
        error: None,
    };
    let (skip_requested, todo_flag) =
        context::subtest_get(sub_id, |s| (s.skip_requested, s.todo)).unwrap_or((false, false));
    if skip_requested {
        res.skipped = true;
        return res;
    }
    if todo_flag {
        res.todo = true;
    }
    let outcome = context::scoped_current(sub_id, || invoke_sub_fn(vm, fn_val));
    context::restore_current_mocks(vm);
    match outcome {
        Err(e) => {
            if is_skip_error(vm, &e) {
                res.skipped = true;
                return res;
            }
            res.passed = false;
            res.error = Some(error_message(vm, &e));
        }
        Ok(()) => {
            if let Some(pe) = context::plan_error(vm) {
                res.passed = false;
                res.error = Some(error_message(vm, &pe));
            }
        }
    }
    // 嵌套子测试失败传播。
    if res.passed && !res.skipped {
        for sr in context::current_sub_results() {
            if !sr.passed {
                res.passed = false;
                if res.error.is_none() {
                    let err = sr.error.clone().unwrap_or_default();
                    res.error = Some(format!("{}: {err}", sr.full_name));
                }
            }
        }
    }
    res
}

/// 子测试函数调用（t 上下文 + async 驱动 + 拒绝近似）。
fn invoke_sub_fn(vm: &mut Vm, fn_val: Value) -> Result<(), VmError> {
    let t = context::new_test_context(vm);
    let result = vm.invoke_callable(fn_val, Value::Undefined, &[t])?;
    if is_promise(vm, result) {
        vm.drain_microtasks()?;
        if promise_rejected(vm, result) {
            let msg = rejection_message(vm, result);
            return Err(VmError::Thrown(Value::Object(vm.alloc_string(msg))));
        }
    }
    Ok(())
}
