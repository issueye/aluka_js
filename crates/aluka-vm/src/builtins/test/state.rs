//! node:test 运行状态表（Phase 8）：用例/子测试状态的线程局部存储与访问器。
//!
//! 移植 Node.js 22 LTS 标准（`nodetest/test_context.go`）的 `testRunState`：
//! plan/asserts 计数、skip/todo 标记、子测试表与子结果收集；以线程局部
//! 单例持有（JS 单线程语义；`cargo test` 并行用例各占线程互不污染），
//! 「当前状态」由运行器经 [`scoped_current`] 作用域化设置，`t.*` 处理器
//! 直接作用于当前状态。

use super::asserts::thrown_msg;
use super::mock;
use super::runner;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;

/// 子测试运行状态。
#[derive(Clone)]
pub struct SubtestState {
    /// 状态 id。
    pub id: u64,
    /// 所属父状态 id。
    pub parent: u64,
    /// 子测试名。
    pub name: String,
    /// 完整名（"parent > child"）。
    pub full: String,
    /// 子测试函数。
    pub fn_val: Value,
    /// t.test 返回的 promise（父 await 用）。
    pub promise: Option<Value>,
    /// 已被父取消（同步父未 await——Node 语义）。
    pub cancelled: bool,
    /// t.skip() 已调用。
    pub skip_requested: bool,
    /// t.todo() 已调用。
    pub todo: bool,
    /// t.plan(n) 期望断言数（0 = 未设置）。
    pub plan: u32,
    /// t.assert 调用计数。
    pub asserts: u32,
    /// 嵌套子测试结果。
    pub sub_results: Vec<runner::TestResult>,
    /// per-test mock 的 spy 槽位（测试结束时自动还原）。
    pub mock_spies: Vec<usize>,
}

/// 单个用例的运行状态。
#[derive(Clone)]
pub struct RunState {
    /// 状态 id。
    pub id: u64,
    /// 用例名。
    pub name: String,
    /// 完整名。
    pub full: String,
    /// 用例函数。
    pub fn_val: Value,
    /// t.plan(n) 期望断言数（0 = 未设置）。
    pub plan: u32,
    /// t.assert 调用计数。
    pub asserts: u32,
    /// t.skip() 已调用。
    pub skip_requested: bool,
    /// t.todo() 已调用。
    pub todo: bool,
    /// 已注册子测试（顺序）。
    pub subtests: Vec<u64>,
    /// 子测试执行结果（失败传播给父）。
    pub sub_results: Vec<runner::TestResult>,
    /// 子测试钩子（t.before——首个子测试前一次）。
    pub before_hooks: Vec<Value>,
    /// 子测试钩子（t.after——末个子测试后一次）。
    pub after_hooks: Vec<Value>,
    /// 子测试间钩子（t.beforeEach——每个子测试前）。
    pub before_each: Vec<Value>,
    /// 子测试间钩子（t.afterEach——每个子测试后）。
    pub after_each: Vec<Value>,
    /// per-test mock 的 spy 槽位（测试结束时自动还原）。
    pub mock_spies: Vec<usize>,
}

thread_local! {
    /// 运行状态表（id → 状态）。
    pub(crate) static RUN_STATES: RefCell<HashMap<u64, RunState>> = RefCell::new(HashMap::new());
    /// 子测试状态表。
    pub(crate) static SUBTEST_STATES: RefCell<HashMap<u64, SubtestState>> =
        RefCell::new(HashMap::new());
    /// 当前正在执行的测试/钩子状态。
    static CURRENT: RefCell<Option<u64>> = const { RefCell::new(None) };
    /// 状态 id 分配器。
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
}

/// 分配新状态 id。
fn next_id() -> u64 {
    NEXT_ID.with(|n| {
        let id = *n.borrow();
        *n.borrow_mut() = id + 1;
        id
    })
}

/// 新建用例运行状态并登记，返回状态 id。
pub fn new_run_state(name: &str, full: &str, fn_val: Value) -> u64 {
    let id = next_id();
    RUN_STATES.with(|m| {
        m.borrow_mut().insert(
            id,
            RunState {
                id,
                name: name.to_owned(),
                full: full.to_owned(),
                fn_val,
                plan: 0,
                asserts: 0,
                skip_requested: false,
                todo: false,
                subtests: Vec::new(),
                sub_results: Vec::new(),
                before_hooks: Vec::new(),
                after_hooks: Vec::new(),
                before_each: Vec::new(),
                after_each: Vec::new(),
                mock_spies: Vec::new(),
            },
        )
    });
    id
}

/// 新建子测试状态并登记，返回状态 id。
pub fn new_subtest_state(parent: u64, name: &str, full: &str, fn_val: Value) -> u64 {
    let id = next_id();
    SUBTEST_STATES.with(|m| {
        m.borrow_mut().insert(
            id,
            SubtestState {
                id,
                parent,
                name: name.to_owned(),
                full: full.to_owned(),
                fn_val,
                promise: None,
                cancelled: false,
                skip_requested: false,
                todo: false,
                plan: 0,
                asserts: 0,
                sub_results: Vec::new(),
                mock_spies: Vec::new(),
            },
        )
    });
    id
}

/// 移除状态（运行结束后清理）。
pub fn drop_state(id: u64) {
    RUN_STATES.with(|m| {
        m.borrow_mut().remove(&id);
    });
    SUBTEST_STATES.with(|m| {
        m.borrow_mut().remove(&id);
    });
}

/// 在指定状态为「当前」的上下文中执行 `f`（保存/恢复——钩子状态隔离）。
pub fn scoped_current<R>(id: u64, f: impl FnOnce() -> R) -> R {
    let saved = CURRENT.with(|c| c.borrow_mut().replace(id));
    let out = f();
    CURRENT.with(|c| *c.borrow_mut() = saved);
    out
}

/// 当前状态 id。
pub fn current_id() -> Option<u64> {
    CURRENT.with(|c| *c.borrow())
}

/// 状态可变视图（父/子状态共通字段的抽象）。
pub enum StateMut<'a> {
    /// 用例状态。
    Run(&'a mut RunState),
    /// 子测试状态。
    Sub(&'a mut SubtestState),
}

impl StateMut<'_> {
    /// 断言计数 +1。
    pub fn add_assert(&mut self) {
        match self {
            StateMut::Run(st) => st.asserts += 1,
            StateMut::Sub(st) => st.asserts += 1,
        }
    }

    /// 标记 skip。
    pub fn mark_skip(&mut self) {
        match self {
            StateMut::Run(st) => st.skip_requested = true,
            StateMut::Sub(st) => st.skip_requested = true,
        }
    }

    /// 标记 todo。
    pub fn mark_todo(&mut self) {
        match self {
            StateMut::Run(st) => st.todo = true,
            StateMut::Sub(st) => st.todo = true,
        }
    }

    /// 设置 plan。
    pub fn set_plan(&mut self, n: u32) {
        match self {
            StateMut::Run(st) => st.plan = n,
            StateMut::Sub(st) => st.plan = n,
        }
    }

    /// 挂接 per-test mock spy 槽位。
    pub fn add_mock_spy(&mut self, slot: usize) {
        match self {
            StateMut::Run(st) => st.mock_spies.push(slot),
            StateMut::Sub(st) => st.mock_spies.push(slot),
        }
    }

    /// 取 per-test mock spy 槽位列表。
    pub fn mock_spies(&self) -> Vec<usize> {
        match self {
            StateMut::Run(st) => st.mock_spies.clone(),
            StateMut::Sub(st) => st.mock_spies.clone(),
        }
    }
}

/// 对当前状态执行 `f`（父或子状态表皆可；不存在时静默跳过）。
pub fn with_current_mut<R>(f: impl FnOnce(&mut StateMut<'_>) -> R) -> Option<R> {
    let id = current_id()?;
    let in_run = RUN_STATES.with(|m| m.borrow().contains_key(&id));
    if in_run {
        RUN_STATES.with(|m| {
            let mut guard = m.borrow_mut();
            guard.get_mut(&id).map(|st| f(&mut StateMut::Run(st)))
        })
    } else {
        SUBTEST_STATES.with(|m| {
            let mut guard = m.borrow_mut();
            guard.get_mut(&id).map(|st| f(&mut StateMut::Sub(st)))
        })
    }
}

/// plan 校验（用例结束时断言数必须等于 plan——Node 语义）。
pub fn plan_error(vm: &mut Vm) -> Option<VmError> {
    let id = current_id()?;
    let pair = RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| (st.plan, st.asserts)))
        .or_else(|| SUBTEST_STATES.with(|m| m.borrow().get(&id).map(|st| (st.plan, st.asserts))))?;
    let (plan, asserts) = pair;
    if plan > 0 && asserts != plan {
        return Some(thrown_msg(
            vm,
            &format!("expected {plan} assertion calls, but received {asserts}"),
        ));
    }
    None
}

/// 读取当前状态的 skip/todo 标记。
pub fn current_flags() -> (bool, bool) {
    let Some(id) = current_id() else {
        return (false, false);
    };
    RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| (st.skip_requested, st.todo)))
        .or_else(|| {
            SUBTEST_STATES.with(|m| m.borrow().get(&id).map(|st| (st.skip_requested, st.todo)))
        })
        .unwrap_or((false, false))
}

/// 当前状态是否已有子测试（同步父取消判定用）。
pub fn current_has_subtests() -> bool {
    let Some(id) = current_id() else {
        return false;
    };
    RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| !st.subtests.is_empty()))
        .unwrap_or(false)
}

/// 当前状态的子测试 id 列表。
pub fn current_subtest_ids() -> Vec<u64> {
    let Some(id) = current_id() else {
        return Vec::new();
    };
    RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| st.subtests.clone()))
        .unwrap_or_default()
}

/// 当前状态子测试结果列表。
pub fn current_sub_results() -> Vec<runner::TestResult> {
    let Some(id) = current_id() else {
        return Vec::new();
    };
    RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| st.sub_results.clone()))
        .or_else(|| SUBTEST_STATES.with(|m| m.borrow().get(&id).map(|st| st.sub_results.clone())))
        .unwrap_or_default()
}

/// 标记子测试被取消（同步父测试结束时调用——Node 语义）。
pub fn cancel_subtests(ids: &[u64]) {
    for &id in ids {
        SUBTEST_STATES.with(|m| {
            if let Some(sub) = m.borrow_mut().get_mut(&id) {
                sub.cancelled = true;
            }
        });
    }
}

/// 注册子测试到当前状态（返回 `(子状态 id, promise)`）。
pub fn attach_subtest(vm: &mut Vm, name: &str, full: &str, fn_val: Value) -> (u64, Value) {
    let parent = current_id().unwrap_or(0);
    let sub_id = new_subtest_state(parent, name, full, fn_val);
    RUN_STATES.with(|m| {
        if let Some(st) = m.borrow_mut().get_mut(&parent) {
            st.subtests.push(sub_id);
        }
    });
    SUBTEST_STATES.with(|m| {
        if let Some(sub) = m.borrow_mut().get_mut(&sub_id) {
            let p = vm.alloc_pending_promise();
            sub.promise = Some(Value::Object(p));
        }
    });
    let promise = SUBTEST_STATES.with(|m| m.borrow().get(&sub_id).and_then(|s| s.promise));
    (sub_id, promise.unwrap_or(Value::Undefined))
}

/// 子测试状态读取。
pub fn subtest_get<R>(id: u64, f: impl FnOnce(&SubtestState) -> R) -> Option<R> {
    SUBTEST_STATES.with(|m| m.borrow().get(&id).map(f))
}

/// 子测试状态修改。
pub fn subtest_mut<R>(id: u64, f: impl FnOnce(&mut SubtestState) -> R) -> Option<R> {
    SUBTEST_STATES.with(|m| {
        let mut guard = m.borrow_mut();
        guard.get_mut(&id).map(f)
    })
}

/// 子测试结果追加到父（父状态或嵌套子状态皆尝试）。
pub fn push_sub_result(parent: u64, result: runner::TestResult) {
    let in_run = RUN_STATES.with(|m| m.borrow().contains_key(&parent));
    if in_run {
        RUN_STATES.with(|m| {
            if let Some(st) = m.borrow_mut().get_mut(&parent) {
                st.sub_results.push(result);
            }
        });
    } else {
        SUBTEST_STATES.with(|m| {
            if let Some(st) = m.borrow_mut().get_mut(&parent) {
                st.sub_results.push(result);
            }
        });
    }
}

/// 当前状态挂接 per-test mock spy。
pub fn current_add_mock_spy(slot: usize) {
    with_current_mut(|st| st.add_mock_spy(slot));
}

/// 还原当前状态的 per-test mock（测试结束时调用——Node 语义）。
pub fn restore_current_mocks(vm: &mut Vm) {
    let slots = with_current_mut(|st| st.mock_spies()).unwrap_or_default();
    for slot in slots {
        mock::restore_slot(vm, slot);
    }
}

/// 完整名拼接（"parent > child"，对齐 Go `joinName`）。
pub fn join_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        return name.to_owned();
    }
    if name.is_empty() {
        return prefix.to_owned();
    }
    format!("{prefix} > {name}")
}

/// 读取指定状态的完整名。
pub fn run_state_full(id: u64) -> String {
    RUN_STATES
        .with(|m| m.borrow().get(&id).map(|st| st.full.clone()))
        .unwrap_or_default()
}

/// 构造 TestContext 对象（`t` 参数；状态绑定当前 CURRENT）。
pub fn new_test_context(vm: &mut Vm) -> Value {
    let (name, full) = RUN_STATES
        .with(|m| {
            current_id().and_then(|id| {
                m.borrow()
                    .get(&id)
                    .map(|st| (st.name.clone(), st.full.clone()))
            })
        })
        .or_else(|| {
            SUBTEST_STATES.with(|m| {
                current_id().and_then(|id| {
                    m.borrow()
                        .get(&id)
                        .map(|st| (st.name.clone(), st.full.clone()))
                })
            })
        })
        .unwrap_or_default();

    let t = vm.alloc_ordinary();
    let t_ns = Value::Object(vm.alloc_string("test:ctx".to_owned()));
    let _ = vm.set_property(Value::Object(t), "_builtinNs", t_ns);
    let name_val = Value::Object(vm.alloc_string(name));
    let _ = vm.set_property(Value::Object(t), "name", name_val);
    let full_val = Value::Object(vm.alloc_string(full));
    let _ = vm.set_property(Value::Object(t), "fullName", full_val);
    // t.filePath：当前测试文件路径（Rust 侧取入口文件；无则为空串）。
    let file_val = Value::Object(vm.alloc_string(vm.entry_file.clone()));
    let _ = vm.set_property(Value::Object(t), "filePath", file_val);
    // t.signal：独立信号对象（aluka 无 AbortSignal 全局——Go 同样回退普通对象）。
    let signal = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(t), "signal", Value::Object(signal));

    // t.assert：断言对象（全部断言递增计数——t.plan 只计 t.assert）。
    let assert_obj = vm.alloc_ordinary();
    let assert_ns = Value::Object(vm.alloc_string("test:ctx.assert".to_owned()));
    let _ = vm.set_property(Value::Object(assert_obj), "_builtinNs", assert_ns);
    for (prop, name) in [
        ("ok", "test:ctx.assert.ok"),
        ("strictEqual", "test:ctx.assert.strictEqual"),
        ("equal", "test:ctx.assert.equal"),
        ("deepStrictEqual", "test:ctx.assert.deepStrictEqual"),
        ("deepEqual", "test:ctx.assert.deepEqual"),
        ("notStrictEqual", "test:ctx.assert.notStrictEqual"),
        ("notEqual", "test:ctx.assert.notEqual"),
        ("notDeepEqual", "test:ctx.assert.notDeepEqual"),
        ("notDeepStrictEqual", "test:ctx.assert.notDeepStrictEqual"),
        ("ifError", "test:ctx.assert.ifError"),
        ("fail", "test:ctx.assert.fail"),
        ("match", "test:ctx.assert.match"),
        ("doesNotMatch", "test:ctx.assert.doesNotMatch"),
        ("throws", "test:ctx.assert.throws"),
        ("rejects", "test:ctx.assert.rejects"),
        ("doesNotReject", "test:ctx.assert.doesNotReject"),
        ("snapshot", "test:ctx.assert.snapshot"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        let _ = vm.set_property(Value::Object(assert_obj), prop, Value::Object(fn_ref));
    }
    // register() 注册的自定义断言挂到 t.assert（Node 22.14 语义）。
    for (name, fn_val) in super::registry::take_custom_asserts() {
        let _ = vm.set_property(Value::Object(assert_obj), &name, fn_val);
    }
    let mock_noop = vm.alloc_native_fn("test:ctx.assert.noop");
    let _ = vm.set_property(Value::Object(assert_obj), "mock", Value::Object(mock_noop));
    let _ = vm.set_property(Value::Object(t), "assert", Value::Object(assert_obj));

    for (prop, name) in [
        ("diagnostic", "test:ctx.diagnostic"),
        ("skip", "test:ctx.skip"),
        ("todo", "test:ctx.todo"),
        ("plan", "test:ctx.plan"),
        ("runOnly", "test:ctx.runOnly"),
        ("test", "test:ctx.test"),
        ("before", "test:ctx.before"),
        ("after", "test:ctx.after"),
        ("beforeEach", "test:ctx.beforeEach"),
        ("afterEach", "test:ctx.afterEach"),
        ("waitFor", "test:ctx.waitFor"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        let _ = vm.set_property(Value::Object(t), prop, Value::Object(fn_ref));
    }
    // t.mock：per-test MockTracker（测试结束时自动还原全部 mock）。
    let tracker = mock::new_tracker(vm, mock::TrackerScope::Scoped);
    let _ = vm.set_property(Value::Object(t), "mock", tracker);
    Value::Object(t)
}
