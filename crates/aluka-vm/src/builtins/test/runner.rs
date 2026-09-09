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

    // 注册顺序执行 children（tests 与 suites 混合）；concurrency 标记的
    // 连续用例组成并发批（M5.4：async 体并发启动、事件循环交错驱动）。
    let children: Vec<Child> = suite.children.clone();
    let suite_concurrent = suite.concurrent;
    let mut idx = 0usize;
    while idx < children.len() {
        match children[idx] {
            Child::Suite(sub) => {
                run_suite(vm, reg, sub, &pfx, results, skip, todo, only);
                idx += 1;
            }
            Child::Test(t) => {
                let concurrent = suite_concurrent || reg.tests[t].concurrent;
                if !concurrent {
                    let name = reg.tests[t].name.clone();
                    let full = join_name(&pfx, &name);
                    if let Some(mut rs) =
                        run_test_case(vm, reg, suite_idx, t, &full, skip, todo, only)
                    {
                        results.append(&mut rs);
                    }
                    idx += 1;
                } else {
                    let mut batch = Vec::new();
                    while idx < children.len() {
                        match children[idx] {
                            Child::Test(t2) if suite_concurrent || reg.tests[t2].concurrent => {
                                batch.push(t2);
                                idx += 1;
                            }
                            _ => break,
                        }
                    }
                    if let Some(mut rs) =
                        run_concurrent_batch(vm, reg, suite_idx, &batch, skip, todo, only)
                    {
                        results.append(&mut rs);
                    }
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
        // 父测试 async：微任务 + 宏任务（定时器等）交替排空驱动 await
        // 直到 promise 落定（M5.4 修复：旧实现只排微任务——`await
        // setTimeout(...)` 之类的宏任务挂起 promise 永不落定，被
        // promise_rejected=false 误判为 Done 假通过）；兑现非 undefined
        // 值按拒绝近似处理（引擎 promise 拒绝同形）。
        let started = std::time::Instant::now();
        loop {
            vm.drain_microtasks()?;
            if !promise_pending(vm, result) {
                break;
            }
            if !vm.macro_tasks.is_empty() || vm.has_active_event_sources() {
                vm.drain_macro_tasks()?;
            } else if vm.microtask_queue.is_empty() {
                // 无驱动工作却未落定：防自旋（宿主 promise 永不兑现场景）
                if started.elapsed() > std::time::Duration::from_secs(120) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        if promise_pending(vm, result) {
            // 120s 仍未落定：判失败，不得静默通过
            return Err(VmError::Thrown(Value::Object(vm.alloc_string(
                "test timed out after 120000ms awaiting promise".to_owned(),
            ))));
        }
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

// ---------------------------------------------------------------------------
// M5.4：并发批执行（concurrency 选项）
// ---------------------------------------------------------------------------

/// 单个并发用例的开始态：start 阶段登记，settle 后收尾。
struct PendingTest {
    /// 套件链（根 → 叶）。
    chain: Vec<usize>,
    /// 运行状态 id（plan/子测试/统计作用域）。
    state_id: u64,
    /// async 体返回的 promise（同步体为 None）。
    promise: Option<Value>,
    /// 同步抛错（start 阶段 invoke 直接 Err）。
    sync_err: Option<VmError>,
    /// 收尾结果（settle 后填）。
    res: TestResult,
}

/// 并发执行一批用例：逐个 start（beforeEach + 调用函数体，async 体在首个
/// await 处挂起），随后事件循环交错驱动直至全部落定，再逐个收尾
/// （plan 校验 + 子测试 + afterEach）。返回整批结果（顺序 = 注册顺序）。
#[allow(clippy::too_many_arguments)]
fn run_concurrent_batch(
    vm: &mut Vm,
    reg: &Registry,
    suite_idx: usize,
    batch: &[usize],
    suite_skip: bool,
    suite_todo: bool,
    only: bool,
) -> Option<Vec<TestResult>> {
    // 收集套件链（根 → 叶）。
    let mut chain: Vec<usize> = Vec::new();
    let mut cur = Some(suite_idx);
    while let Some(idx) = cur {
        chain.push(idx);
        cur = reg.suites[idx].parent;
    }
    chain.reverse();

    let mut out: Vec<TestResult> = Vec::new();
    let mut pending: Vec<PendingTest> = Vec::new();

    // ---- start 阶段：按注册顺序启动全部用例 ----
    for &test_idx in batch {
        let tc = reg.tests[test_idx].clone();
        if only && !tc.only {
            continue;
        }
        let full = suite_test_full_name(reg, suite_idx, &tc.name);
        if suite_skip || tc.skip {
            out.push(TestResult {
                name: tc.name.clone(),
                full_name: full,
                passed: true,
                skipped: true,
                todo: false,
                cancelled: false,
                error: None,
            });
            continue;
        }
        let is_todo = tc.todo || suite_todo;
        let mut res = TestResult {
            name: tc.name.clone(),
            full_name: full.clone(),
            passed: true,
            skipped: false,
            todo: is_todo,
            cancelled: false,
            error: None,
        };
        // beforeEach（外层 → 内层）：失败 → 该用例标失败并跳过函数体。
        let mut hook_err: Option<String> = None;
        for &s in &chain {
            let hooks: Vec<Value> = reg.suites[s].before_each.clone();
            for h in hooks {
                if let Err(e) = invoke_hook_fn(vm, h) {
                    hook_err = Some(format!("beforeEach: {}", error_message(vm, &e)));
                    res.passed = false;
                    res.error = hook_err.clone();
                    break;
                }
            }
            if hook_err.is_some() {
                break;
            }
        }
        if hook_err.is_some() {
            out.push(res);
            continue;
        }

        // 调用函数体（t 参数）；async 体不在此排空——promise 挂起交由
        // settle 阶段交错驱动（并发语义核心）。
        // M5.4 修复：start 必须在 scoped_current 内进行——
        // ① t 的 _stateId 绑定需要 CURRENT 指向本用例状态；
        // ② 同步段的 plan/assert 也经 CURRENT 归位。
        // await 之后（settle 段）CURRENT 会被同批其它用例占据，届时经
        // t/_stateId（receiver 绑定）找回——两条路径缺一不可。
        let state_id = context::new_run_state(&tc.name, &full, tc.fn_val);
        let call_result = context::scoped_current(state_id, || {
            let t = context::new_test_context(vm);
            vm.invoke_callable(tc.fn_val, Value::Undefined, &[t])
        });
        let (promise, sync_err) = match call_result {
            Ok(v) if is_promise(vm, v) => (Some(v), None),
            Ok(_) => (None, None),
            Err(e) => (None, Some(e)),
        };
        pending.push(PendingTest {
            chain: chain.clone(),
            state_id,
            promise,
            sync_err,
            res,
        });
    }

    // ---- settle 阶段：微任务/宏任务交替排空，直至全部 promise 落定 ----
    //（调用方 run_suite 返回 Option；错误在此转标记，防 `?` 型不匹配）
    let mut settle_err: Option<VmError> = None;
    let mut settle_timeout = false;
    let started = std::time::Instant::now();
    loop {
        let all_settled = pending
            .iter()
            .all(|p| p.promise.is_none_or(|pv| !promise_pending(vm, pv)));
        if all_settled {
            break;
        }
        if let Err(e) = vm.drain_microtasks() {
            settle_err = Some(e);
            break;
        }
        let all_settled = pending
            .iter()
            .all(|p| p.promise.is_none_or(|pv| !promise_pending(vm, pv)));
        if all_settled {
            break;
        }
        if !vm.macro_tasks.is_empty() || vm.has_active_event_sources() {
            if let Err(e) = vm.drain_macro_tasks() {
                settle_err = Some(e);
                break;
            }
        } else if vm.microtask_queue.is_empty() {
            // 无驱动工作却未落定（宿主 promise 永不兑现）：超时判失败——
            // 挂起 promise 的用例不得静默通过（M5.4 修复：假通过缺陷）。
            if started.elapsed() > std::time::Duration::from_secs(120) {
                settle_timeout = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    if let Some(e) = settle_err {
        for p in &mut pending {
            p.res.passed = false;
            p.res.error = Some(error_message(vm, &e));
        }
    } else if settle_timeout {
        for p in &mut pending {
            if p.promise.is_some_and(|pv| promise_pending(vm, pv)) {
                p.res.passed = false;
                p.res.error = Some("test timed out after 120000ms awaiting promise".to_owned());
            }
        }
    }

    // ---- 收尾阶段：按注册顺序逐个固化结果 ----
    for mut p in pending {
        let snapshot: InvokeSnapshot = context::scoped_current(p.state_id, || {
            // t.mock 的 spy 在测试结束时自动还原（Node 语义）。
            context::restore_current_mocks(vm);
            if let Some(err) = p.sync_err.clone() {
                return InvokeSnapshot::Error(err);
            }
            if let Some(pv) = p.promise {
                if promise_rejected(vm, pv) {
                    let msg = rejection_message(vm, pv);
                    return InvokeSnapshot::Error(VmError::Thrown(Value::Object(
                        vm.alloc_string(msg),
                    )));
                }
            }
            InvokeSnapshot::Done(context::plan_error(vm), context::current_sub_results())
        });
        context::drop_state(p.state_id);
        let (mut invoke_err, plan_err, sub_results) = match snapshot {
            InvokeSnapshot::Error(e) => (Some(e), None, Vec::new()),
            InvokeSnapshot::Cancelled(_) => (None, None, Vec::new()),
            InvokeSnapshot::Done(pe, sub_results) => (None, pe, sub_results),
        };
        if let Some(e) = invoke_err.take() {
            if is_skip_error(vm, &e) {
                p.res.skipped = true;
            } else {
                p.res.passed = false;
                p.res.error = Some(error_message(vm, &e));
            }
        } else if let Some(pe) = plan_err {
            p.res.passed = false;
            p.res.error = Some(error_message(vm, &pe));
        }
        if p.res.passed && !p.res.skipped {
            for sr in &sub_results {
                if !sr.passed {
                    p.res.passed = false;
                    if p.res.error.is_none() {
                        let err = sr.error.clone().unwrap_or_default();
                        p.res.error = Some(format!("{}: {err}", sr.full_name));
                    }
                }
            }
        }
        // afterEach（内层 → 外层）。
        for &s in p.chain.iter().rev() {
            let hooks: Vec<Value> = reg.suites[s].after_each.clone();
            for h in hooks {
                if let Err(e) = invoke_hook_fn(vm, h) {
                    p.res.passed = false;
                    p.res.error = Some(format!("afterEach: {}", error_message(vm, &e)));
                    break;
                }
            }
        }
        let mut block = vec![p.res];
        block.extend(sub_results);
        out.extend(block);
    }
    Some(out)
}

/// 套件内用例的完整名（"root > suite > case"）。
fn suite_test_full_name(reg: &Registry, suite_idx: usize, name: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Some(suite_idx);
    while let Some(idx) = cur {
        parts.push(reg.suites[idx].name.clone());
        cur = reg.suites[idx].parent;
    }
    parts.reverse();
    let mut pfx = String::new();
    for part in parts {
        pfx = join_name(&pfx, &part);
    }
    join_name(&pfx, name)
}

/// promise 是否仍挂起（settle 判定）。
fn promise_pending(vm: &Vm, pv: Value) -> bool {
    matches!(pv, Value::Object(r)
    if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Promise { pending: true, .. })
    ))
}
