//! node:test mock 面（Phase 8）：函数/方法 spy、MockTracker 与 `mock.timers` 假时钟。
//!
//! 移植 Node.js 22 LTS 标准（`nodetest/`）的 MockTracker 表面：
//! `fn` / `method` / `getter` / `setter` / `property` / `timers` / `restoreAll` /
//! `reset`。spy 函数以「固定槽位 trampoline 池」实现（Rust 处理器为 fn 指针、
//! 无闭包捕获——每槽位一个常量泛型实例化，状态存于线程局部表）。
//!
//! 已知限制：spy 为原生函数对象（无属性表），Node 的 `spy.mock.calls`
//! 观测面不可达——调用记录保存在引擎内侧（`MockTracker` 语义），`restore`
//! /委托/`mockImplementation` 行为完整。
//!
//! # `mock.timers`（M5.4 切片二：假时钟）
//!
//! 逐条对齐 Node 22.23.1 的 `internal/test_runner/mock/mock_timers.js`：
//! `enable({ apis, now })`（apis 默认 `['setTimeout','setInterval','setImmediate']`、
//! now 默认 0；重复 enable 抛 `ERR_INVALID_STATE`，未知 api 名抛
//! `ERR_INVALID_ARG_VALUE`）、`tick(time = 1)`（推进假时钟后按
//! `(runAt, id)` 升序执行到期回调，周期任务重排）、`setTime(time = 0)`
//! （**只设时间、不执行回调**）、`runAll()`（≡ Node
//! `tick(最晚到期任务.runAt - now)`）、`reset()`（未启用时 no-op）。
//! `mock.reset()` ≡ Node `restoreAll() + timers.reset()`，同样复位假时钟。
//!
//! 拦截点有两处（两处都必须拦截，否则部分调用会绕过假时钟）：
//! 1. [`crate::builtins::timers`] 的 `schedule_raw`（`require('timers')`、
//!    全局 `timers/promises` 的注册点）；
//! 2. `interpreter.rs` 全局 `setTimeout`/`setInterval`/`setImmediate` 的内联分发。
//!
//! 两处都只调 [`fake_schedule`]/[`fake_clear`]；假时钟队列**不写 `macro_tasks`**，
//! 因此既不真调度也不走 `wait_until_due` 的真 `sleep`。
//!
//! ## 已登记的不支持面（不静默假装支持）
//! - `apis: ['Date']`：**未实现**——`Date.now()` / `new Date()` 仍为真实时间；
//!   api 名通过校验（Node 不报错）但不产生任何副作用。
//! - `apis: ['scheduler.wait']`：**未实现**（同上）。
//! - 定时器句柄为**数字 id**（与引擎真实定时器一致；Node 返回 Timeout 对象）——
//!   `clearTimeout(id)`/`clearInterval(id)` 仍可用。
//! - 假时钟是**引擎级单例**：`t.mock.timers` 与模块级 `mock.timers` 控制同一份
//!   时钟（Node 为每 tracker 独立实例），且测试结束不自动 `reset()`（需显式调用）。
//! - 错误对象为 Error + `.code`（Node 为 `TypeError` 子类实例）。
//! - `tick`/`setTime` 的入参按整数毫秒截断（Node 接受浮点）。

use super::context;
use crate::builtins::BuiltinHandler;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::cell::RefCell;

/// spy 池容量（每槽位一个 trampoline）。
const SPY_POOL_SIZE: usize = 32;

/// tracker 归属：模块级全局 or per-test 作用域。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrackerScope {
    /// 模块级 `mock`（不自动还原）。
    Global,
    /// `t.mock`（测试结束时自动还原）。
    Scoped,
}

/// spy 状态（槽位表条目）。
#[derive(Clone)]
pub struct SpyState {
    /// spy 函数名（mock.method 的方法名）。
    pub method: String,
    /// 被替换的目标对象（`mock.fn` 创建的独立函数无 target）。
    pub target: Option<Value>,
    /// 原始属性值/实现。
    pub original: Value,
    /// mockImplementation 替换实现。
    pub impl_val: Value,
    /// mockImplementationOnce 单次实现。
    pub once_impl: Value,
    /// 是否 `mock.fn` 独立函数（restore 时不写回 target）。
    pub is_fn: bool,
    /// tracker 归属。
    pub scope: TrackerScope,
    /// `.mock` 观测面对象（挂于 spy 函数属性；None = 未挂载）。
    pub mock_obj: Option<aluka_core::ObjectRef>,
    /// `.mock.calls` 数组对象（调用记录按序追加）。
    pub calls_arr: Option<aluka_core::ObjectRef>,
}

thread_local! {
    /// spy 槽位表（None = 空槽）。
    static SPY_STORE: RefCell<Vec<Option<SpyState>>> = const { RefCell::new(Vec::new()) };
}

/// 分配空 spy 槽位。
fn alloc_slot() -> Option<usize> {
    SPY_STORE.with(|s| {
        let mut guard = s.borrow_mut();
        if guard.len() < SPY_POOL_SIZE {
            guard.push(None);
            Some(guard.len() - 1)
        } else {
            guard.iter().position(|slot| slot.is_none())
        }
    })
}

/// 读取 spy 槽位。
fn with_slot<R>(slot: usize, f: impl FnOnce(&SpyState) -> R) -> Option<R> {
    SPY_STORE.with(|s| s.borrow().get(slot).and_then(|o| o.as_ref()).map(f))
}

/// 修改 spy 槽位（槽位须已占用）。
fn with_slot_mut<R>(slot: usize, f: impl FnOnce(&mut SpyState) -> R) -> Option<R> {
    SPY_STORE.with(|s| {
        let mut guard = s.borrow_mut();
        guard.get_mut(slot).and_then(|o| o.as_mut()).map(f)
    })
}

/// 写入 spy 槽位（覆盖占位 None）。
fn set_slot(slot: usize, state: SpyState) {
    SPY_STORE.with(|s| {
        let mut guard = s.borrow_mut();
        if let Some(o) = guard.get_mut(slot) {
            *o = Some(state);
        }
    });
}

/// 泄漏 spy 状态（克隆快照；槽位按值访问规避借用冲突）。
fn take_slot(slot: usize) -> Option<SpyState> {
    with_slot(slot, Clone::clone)
}

/// 常量泛型 trampoline：槽位 N 的 spy 调用入口。
fn spy_trampoline<const N: usize>(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    spy_call(vm, N, args)
}

macro_rules! spy_pool {
    ($($i:literal),*) => {
        /// 槽位 → 处理器表（构建期注册到分派表）。
        pub static SPY_TRAMPS: [BuiltinHandler; SPY_POOL_SIZE] = [
            $(spy_trampoline::<$i>),*
        ];
    };
}

spy_pool!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31
);

/// spy 调用：记录 arguments → 委托 once_impl/impl/original（保持 this——
/// Node 语义）→ 回填 result → 追加 `.mock.calls` 记录。
fn spy_call(vm: &mut Vm, slot: usize, args: &[Value]) -> Result<Value, VmError> {
    let state = match take_slot(slot) {
        Some(s) => s,
        None => return Ok(Value::Undefined),
    };
    // 实现解析：onceImpl（一次性，用后还原）> impl > original。
    let cur_impl = if !matches!(state.once_impl, Value::Undefined) {
        let once = state.once_impl;
        with_slot_mut(slot, |s| s.once_impl = Value::Undefined);
        once
    } else if !matches!(state.impl_val, Value::Undefined) {
        state.impl_val
    } else {
        Value::Undefined
    };
    let this_val = crate::builtins::current_receiver();
    let target = if !matches!(cur_impl, Value::Undefined) {
        cur_impl
    } else {
        state.original
    };
    if !super::is_function_value(vm, target) {
        record_call(vm, &state, args, Value::Undefined, None, this_val, target);
        return Ok(Value::Undefined);
    }
    match vm.invoke_callable(target, this_val, args) {
        Ok(result) => {
            record_call(vm, &state, args, result, None, this_val, target);
            Ok(result)
        }
        Err(VmError::Thrown(exc)) => {
            record_call(
                vm,
                &state,
                args,
                Value::Undefined,
                Some(exc),
                this_val,
                target,
            );
            Err(VmError::Thrown(exc))
        }
        Err(e) => Err(e),
    }
}

/// 追加一条调用记录到 `.mock.calls` 数组对象（就地 push，引用稳定）。
fn record_call(
    vm: &mut Vm,
    state: &SpyState,
    args: &[Value],
    result: Value,
    error: Option<Value>,
    this_val: Value,
    target: Value,
) {
    let (Some(_mock_obj), Some(calls_arr)) = (state.mock_obj, state.calls_arr) else {
        return;
    };
    // 记录对象：{ arguments, error, result, stack, target, this }
    let record = vm.alloc_ordinary();
    let args_arr = vm.alloc_array(args.to_vec());
    let _ = vm.set_property(Value::Object(record), "arguments", Value::Object(args_arr));
    let _ = vm.set_property(
        Value::Object(record),
        "error",
        error.unwrap_or(Value::Undefined),
    );
    let _ = vm.set_property(Value::Object(record), "result", result);
    let stack_str = vm.alloc_string(String::new());
    let _ = vm.set_property(Value::Object(record), "stack", Value::Object(stack_str));
    let _ = vm.set_property(Value::Object(record), "target", target);
    let _ = vm.set_property(Value::Object(record), "this", this_val);
    // 追加进 calls 数组对象
    if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(calls_arr.0 as usize) {
        elements.push(Value::Object(record));
    }
}

// ---------------------------------------------------------------------------
// M5.4 切片二：`mock.timers` 假时钟
// ---------------------------------------------------------------------------

/// Node `internal/timers` 的 `TIMEOUT_MAX`（`#createTimer` 超界延时夹到 1ms 的阈值）。
const TIMEOUT_MAX: u64 = 2_147_483_647;

/// 假时钟可接管的定时器面（Node `SUPPORTED_APIS` 中本仓已实现的子集；
/// `'Date'` / `'scheduler.wait'` 见模块头「已登记的不支持面」）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FakeApi {
    /// `setTimeout` / `clearTimeout` / `timers/promises.setTimeout`。
    SetTimeout,
    /// `setInterval` / `clearInterval`。
    SetInterval,
    /// `setImmediate` / `clearImmediate` / `timers/promises.setImmediate`。
    SetImmediate,
}

/// 假定时器条目（对齐 Node `Timeout` 的 `id` / `runAt` / `interval` / `callback`）。
#[derive(Clone)]
struct FakeTimer {
    /// 句柄 id（假时钟自增，Node `#currentTimer` 从 1 起）。
    id: u64,
    /// 到期假时间（epoch 毫秒；`setImmediate` 为 `now - 1`，对齐 Node）。
    run_at: i64,
    /// 周期（`setInterval` 的延时；其余为 `None`）。
    interval: Option<u64>,
    /// 回调值。
    cb: Value,
}

/// 假时钟状态（Node `MockTimers` 实例；本仓为引擎级单例）。
struct FakeClock {
    /// 是否已 `enable`。
    enabled: bool,
    /// 当前假时间（epoch 毫秒；`reset` 归 0）。
    now: i64,
    /// 被接管的 api 面。
    apis: Vec<FakeApi>,
    /// 待执行队列（线性扫描取最小 `(run_at, id)`——即 Node 优先队列比较器）。
    queue: Vec<FakeTimer>,
    /// 下一个句柄 id。
    next_id: u64,
}

impl FakeClock {
    /// 初始状态（未启用、now = 0、id 从 1 起——Node `kInitialEpoch` / `#currentTimer`）。
    fn new() -> Self {
        Self {
            enabled: false,
            now: 0,
            apis: Vec::new(),
            queue: Vec::new(),
            next_id: 1,
        }
    }

    /// 该 api 是否由假时钟接管。
    fn intercepts(&self, api: FakeApi) -> bool {
        self.enabled && self.apis.contains(&api)
    }
}

thread_local! {
    /// 假时钟（`mock.timers`；单例，见模块头限制）。
    static FAKE_CLOCK: RefCell<FakeClock> = RefCell::new(FakeClock::new());
}

/// 假时钟接管判定 + 登记（`setTimeout`/`setInterval`/`setImmediate` 两个真实
/// 注册点都会调用）。
///
/// 返回 `Some(id)`：假时钟已接管，调用方**不得**再写入 `macro_tasks`
/// （既不真调度也不真 `sleep`）；返回 `None`：该 api 未被 `enable`，
/// 调用方按原有真实定时器语义继续。
pub fn fake_schedule(cb: Value, delay_ms: u64, api: FakeApi) -> Option<Value> {
    FAKE_CLOCK.with(|c| {
        let mut clock = c.borrow_mut();
        if !clock.intercepts(api) {
            return None;
        }
        // 对齐 Node `#createTimer`：setImmediate 用 -1 作延时（runAt = now - 1，
        // 故 tick(0) 即到期）；超出 TIMEOUT_MAX 的延时夹到 1ms。
        let delay: i64 = match api {
            FakeApi::SetImmediate => -1,
            _ if delay_ms > TIMEOUT_MAX => 1,
            _ => i64::try_from(delay_ms).unwrap_or(1),
        };
        let id = clock.next_id;
        clock.next_id += 1;
        let interval = matches!(api, FakeApi::SetInterval).then_some(delay.max(0) as u64);
        let run_at = clock.now + delay;
        clock.queue.push(FakeTimer {
            id,
            run_at,
            interval,
            cb,
        });
        Some(Value::Number(id as f64))
    })
}

/// 假时钟清除接管（`clearTimeout`/`clearInterval`/`clearImmediate` 的调用点调用）。
///
/// 返回 `true`：清除请求已由假时钟处理（Node `#clearTimer` 只认假句柄，
/// 传入真实句柄 = no-op）；`false`：该 api 未被接管，调用方走真实清除路径。
pub fn fake_clear(id: u64, api: FakeApi) -> bool {
    FAKE_CLOCK.with(|c| {
        let mut clock = c.borrow_mut();
        if !clock.intercepts(api) {
            return false;
        }
        clock.queue.retain(|t| t.id != id);
        true
    })
}

/// 假时钟复位（`mock.timers.reset()` 与 `mock.reset()` 共用；未启用时 no-op）。
fn reset_clock() {
    FAKE_CLOCK.with(|c| {
        let mut clock = c.borrow_mut();
        if clock.enabled {
            *clock = FakeClock::new();
        }
    });
}

/// 假时钟是否已启用。
fn clock_enabled() -> bool {
    FAKE_CLOCK.with(|c| c.borrow().enabled)
}

/// 构造 Node 风格错误码异常（Error + `.code`；Node 为 `TypeError` 子类实例）。
fn code_error(vm: &mut Vm, code: &str, msg: &str) -> VmError {
    let err = vm.alloc_error_instance(msg);
    let code_val = Value::Object(vm.alloc_string(code.to_owned()));
    let _ = vm.set_property(Value::Object(err), "code", code_val);
    VmError::Thrown(Value::Object(err))
}

/// `tick`/`setTime`/`enable` 的时间入参校验（Node `#assertTimeArg`：负数即抛）。
fn time_arg_error(vm: &mut Vm, time: i64) -> VmError {
    code_error(
        vm,
        "ERR_INVALID_ARG_VALUE",
        &format!("The argument 'time' {time}. Received 'positive integer'"),
    )
}

/// 未启用时的错误（Node `#assertTimersAreEnabled`）。
fn not_enabled_error(vm: &mut Vm) -> VmError {
    code_error(
        vm,
        "ERR_INVALID_STATE",
        "You should enable MockTimers first by calling the .enable function",
    )
}

/// 读取时间入参（缺省 `default`；非数字/负数按 Node 抛错）。
fn time_arg(vm: &mut Vm, v: Option<Value>, default: i64) -> Result<i64, VmError> {
    match v {
        None | Some(Value::Undefined) => Ok(default),
        Some(Value::Number(n)) => {
            if n.is_nan() {
                return Err(code_error(
                    vm,
                    "ERR_INVALID_ARG_VALUE",
                    "The argument 'time' is not a positive integer",
                ));
            }
            let t = n as i64;
            if t < 0 {
                return Err(time_arg_error(vm, t));
            }
            Ok(t)
        }
        Some(other) => Err(code_error(
            vm,
            "ERR_INVALID_ARG_TYPE",
            &format!(
                "The \"time\" argument must be of type number. Received {}",
                vm.format_value(other)
            ),
        )),
    }
}

/// 推进假时钟 `delta` 毫秒并按 `(runAt, id)` 升序执行到期回调
/// （Node `tick` 主体：`#now += time` 后反复取队内最小任务，`runAt > now` 即停；
/// 周期任务 `runAt += interval` 后重排，回调内被 clear 则不重排）。
///
/// **不调用 `wait_until_due`**：假时钟只做逻辑时间推进，不产生任何真实 `sleep`。
fn advance_clock(vm: &mut Vm, delta: i64) -> Result<(), VmError> {
    let target = FAKE_CLOCK.with(|c| {
        let mut clock = c.borrow_mut();
        clock.now += delta;
        clock.now
    });
    loop {
        let picked = FAKE_CLOCK.with(|c| {
            let clock = c.borrow();
            let mut best: Option<&FakeTimer> = None;
            for t in clock.queue.iter() {
                if t.run_at > target {
                    continue;
                }
                let better = best
                    .map(|b| (t.run_at, t.id) < (b.run_at, b.id))
                    .unwrap_or(true);
                if better {
                    best = Some(t);
                }
            }
            best.cloned()
        });
        let Some(timer) = picked else { break };
        vm.invoke_callable(timer.cb, Value::Undefined, &[])?;
        FAKE_CLOCK.with(|c| {
            let mut clock = c.borrow_mut();
            // 回调期间句柄仍在队列（Node peek 语义）：被 clear 则整条不见；
            // 仍在则摘除，周期任务按 interval 重排。
            if let Some(pos) = clock.queue.iter().position(|t| t.id == timer.id) {
                let mut done = clock.queue.remove(pos);
                if let Some(interval) = done.interval {
                    done.run_at += i64::try_from(interval).unwrap_or(i64::MAX);
                    clock.queue.push(done);
                }
            }
        });
    }
    Ok(())
}

/// 构造 `mock.timers` 对象（Node `MockTimers` 实例表面；分派命名空间
/// `test:mockTimers`，见 `builtins::try_dispatch` 形态二）。
fn new_timers_object(vm: &mut Vm) -> Value {
    let obj = vm.alloc_ordinary();
    let ns_val = Value::Object(vm.alloc_string("test:mockTimers".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", ns_val);
    for (prop, name) in [
        ("enable", "test:mockTimers.enable"),
        ("tick", "test:mockTimers.tick"),
        ("setTime", "test:mockTimers.setTime"),
        ("runAll", "test:mockTimers.runAll"),
        ("reset", "test:mockTimers.reset"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        let _ = vm.set_property(Value::Object(obj), prop, Value::Object(fn_ref));
    }
    Value::Object(obj)
}

/// `mock.timers.enable({ apis, now })`：按 `apis` 接管全局定时器，`now` 为起始假时间
/// （Node 语义：apis 缺省三面全开、now 缺省 0；重复 enable 抛 `ERR_INVALID_STATE`）。
fn timers_enable(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if clock_enabled() {
        return Err(code_error(
            vm,
            "ERR_INVALID_STATE",
            "MockTimers is already enabled!",
        ));
    }
    let opts = args.first().copied().unwrap_or(Value::Undefined);
    let apis_val = vm.get_property(opts, "apis").unwrap_or(Value::Undefined);
    let mut apis = vec![
        FakeApi::SetTimeout,
        FakeApi::SetInterval,
        FakeApi::SetImmediate,
    ];
    if !matches!(apis_val, Value::Undefined) {
        let Value::Object(r) = apis_val else {
            return Err(code_error(
                vm,
                "ERR_INVALID_ARG_TYPE",
                "The \"options.apis\" property must be of type Array.",
            ));
        };
        apis.clear();
        for v in vm.array_elements(r.index()) {
            match vm.format_value(v).as_str() {
                "setTimeout" => apis.push(FakeApi::SetTimeout),
                "setInterval" => apis.push(FakeApi::SetInterval),
                "setImmediate" => apis.push(FakeApi::SetImmediate),
                // Node 的 SUPPORTED_APIS 含 'Date' / 'scheduler.wait'（本仓未实现，
                // 见模块头「已登记的不支持面」）：名字通过校验但不产生副作用。
                "Date" | "scheduler.wait" => {}
                other => {
                    return Err(code_error(
                        vm,
                        "ERR_INVALID_ARG_VALUE",
                        &format!(
                            "The argument 'options.apis' {other} is not supported. \
                             Received 'setTimeout, setInterval, setImmediate'"
                        ),
                    ));
                }
            }
        }
    }
    let now = match vm.get_property(opts, "now").unwrap_or(Value::Undefined) {
        Value::Undefined => 0,
        Value::Number(n) if !n.is_nan() => {
            let t = n as i64;
            if t < 0 {
                return Err(time_arg_error(vm, t));
            }
            t
        }
        other => {
            return Err(code_error(
                vm,
                "ERR_INVALID_ARG_TYPE",
                &format!(
                    "The \"options.now\" property must be of type number. Received {}",
                    vm.format_value(other)
                ),
            ));
        }
    };
    FAKE_CLOCK.with(|c| {
        let mut clock = c.borrow_mut();
        clock.enabled = true;
        clock.now = now;
        clock.apis = apis;
        clock.queue.clear();
        clock.next_id = 1;
    });
    Ok(Value::Undefined)
}

/// `mock.timers.tick(time = 1)`：推进假时钟并执行到期回调。
fn timers_tick(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if !clock_enabled() {
        return Err(not_enabled_error(vm));
    }
    let time = time_arg(vm, args.first().copied(), 1)?;
    advance_clock(vm, time)?;
    Ok(Value::Undefined)
}

/// `mock.timers.setTime(time = 0)`：只设当前假时间，**不执行**任何回调（Node 语义）。
fn timers_set_time(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if !clock_enabled() {
        return Err(not_enabled_error(vm));
    }
    let time = time_arg(vm, args.first().copied(), 0)?;
    FAKE_CLOCK.with(|c| c.borrow_mut().now = time);
    Ok(Value::Undefined)
}

/// `mock.timers.runAll()`：Node 语义为 `tick(队内最晚到期任务.runAt - now)`，
/// 无任务时 no-op。
///
/// 注意（Node 22.23.1 实测 + `mock_timers.js` 源码锚定）：若最晚到期任务**早于**
/// 当前假时间（先 `setTime` 往后跳再 `runAll`），Node 会以负增量调用 `tick` 并抛
/// `ERR_INVALID_ARG_VALUE`；本实现同样抛出——不发明「补齐执行过期任务」的语义。
fn timers_run_all(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    if !clock_enabled() {
        return Err(not_enabled_error(vm));
    }
    let (longest, now) = FAKE_CLOCK.with(|c| {
        let clock = c.borrow();
        (clock.queue.iter().map(|t| t.run_at).max(), clock.now)
    });
    let Some(longest) = longest else {
        return Ok(Value::Undefined);
    };
    let delta = longest - now;
    if delta < 0 {
        return Err(time_arg_error(vm, delta));
    }
    advance_clock(vm, delta)?;
    Ok(Value::Undefined)
}

/// `mock.timers.reset()`：还原被接管的全局定时器（清假时钟）、清空队列、
/// 时间归 0；未启用时 no-op（Node 语义）。
fn timers_reset(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    reset_clock();
    Ok(Value::Undefined)
}

/// MockTracker 构造（模块级 `mock` 与 per-test `t.mock` 共用）。
pub fn new_tracker(vm: &mut Vm, scope: TrackerScope) -> Value {
    let mock_obj = vm.alloc_ordinary();
    let ns_val = Value::Object(vm.alloc_string("test:mock".to_owned()));
    let _ = vm.set_property(Value::Object(mock_obj), "_builtinNs", ns_val);
    for (prop, name) in [
        ("fn", "test:mock.fn"),
        ("method", "test:mock.method"),
        ("getter", "test:mock.getter"),
        ("setter", "test:mock.setter"),
        ("property", "test:mock.property"),
        ("restoreAll", "test:mock.restoreAll"),
        ("reset", "test:mock.reset"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        let _ = vm.set_property(Value::Object(mock_obj), prop, Value::Object(fn_ref));
    }
    // timers：`mock.timers` 假时钟面（Node MockTracker 的 `timers` getter）。
    let timers_obj = new_timers_object(vm);
    let _ = vm.set_property(Value::Object(mock_obj), "timers", timers_obj);
    let _ = scope;
    Value::Object(mock_obj)
}

/// 记录新 spy（分配槽位并登记分派），并挂 `.mock` 观测面对象。
fn install_spy(vm: &mut Vm, mut state: SpyState) -> Result<Value, VmError> {
    let Some(slot) = alloc_slot() else {
        return Err(super::asserts::thrown_msg(
            vm,
            "mock tracker spy pool exhausted (max 32)",
        ));
    };
    // `.mock` 观测面：{ calls: [...] }（数组对象引用稳定，调用后就地追加）
    let calls_arr = vm.alloc_array(Vec::new());
    let mock_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(mock_obj), "calls", Value::Object(calls_arr));
    let restore_ref = vm.alloc_native_fn("test:mock.restoreSpySlot");
    let _ = vm.set_property(
        Value::Object(mock_obj),
        "restore",
        Value::Object(restore_ref),
    );
    state.mock_obj = Some(mock_obj);
    state.calls_arr = Some(calls_arr);
    set_slot(slot, state);
    register_handler_at(vm, slot);
    let fn_ref = vm.alloc_native_fn(&format!("test:mockSpy.{slot}"));
    vm.set_native_fn_property(fn_ref, "mock", Value::Object(mock_obj));
    // per-test tracker：挂到当前状态（测试结束自动还原——Node 语义）。
    if take_slot(slot).map(|s| s.scope) == Some(TrackerScope::Scoped) {
        context::current_add_mock_spy(slot);
    }
    Ok(Value::Object(fn_ref))
}

/// 运行期把槽位处理器登记进分派表（`test:mockSpy.{slot}` → trampoline）。
fn register_handler_at(vm: &mut Vm, slot: usize) {
    let handler = SPY_TRAMPS[slot];
    crate::builtins::register_handler(
        &mut vm.builtin_registry,
        "test:mockSpy",
        &slot.to_string(),
        handler,
    );
}

/// `mock.fn([impl])`：独立 spy 函数。
fn mock_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let impl_val = match args.first() {
        Some(v) if super::is_function_value(vm, *v) => *v,
        _ => Value::Undefined,
    };
    install_spy(
        vm,
        SpyState {
            method: String::new(),
            target: None,
            original: impl_val,
            impl_val,
            once_impl: Value::Undefined,
            is_fn: true,
            scope: tracker_scope_of(),
            mock_obj: None,
            calls_arr: None,
        },
    )
}

/// 读取「正在创建 tracker 的作用域」：t.mock 的创建发生在测试上下文构造中
/// （CURRENT 已绑定），模块级 mock 在其外。
fn tracker_scope_of() -> TrackerScope {
    if context::current_id().is_some() {
        TrackerScope::Scoped
    } else {
        TrackerScope::Global
    }
}

/// `mock.method(target, name[, impl])`：替换对象方法为 spy。
fn mock_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(super::asserts::type_fail(
            vm,
            "mock.method(target, methodName)",
        ));
    }
    let target = match args.first().copied() {
        Some(v) if super::is_function_value(vm, v) || is_ordinary(vm, v) => v,
        _ => {
            return Err(super::asserts::type_fail(
                vm,
                "mock.method target must be an object",
            ));
        }
    };
    let method = vm.format_value(args[1]);
    let original = vm.get_property(target, &method).unwrap_or(Value::Undefined);
    let impl_val = match args.get(2) {
        Some(v) if super::is_function_value(vm, *v) => *v,
        _ => Value::Undefined,
    };
    let spy = install_spy(
        vm,
        SpyState {
            method: method.clone(),
            target: Some(target),
            original,
            impl_val,
            once_impl: Value::Undefined,
            is_fn: false,
            scope: tracker_scope_of(),
            mock_obj: None,
            calls_arr: None,
        },
    )?;
    let _ = vm.set_property(target, &method, spy);
    Ok(spy)
}

/// 是否普通对象。
fn is_ordinary(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r)
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })))
}

/// `mock.getter(target, name[, impl])`：近似移植——以当前值/实现写入
/// 数据属性（引擎访问器仅支持编译期函数模板）。
fn mock_getter(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(super::asserts::type_fail(
            vm,
            "mock.getter(target, property)",
        ));
    }
    let target = match args.first().copied() {
        Some(v) if is_ordinary(vm, v) => v,
        _ => {
            return Err(super::asserts::type_fail(
                vm,
                "mock.getter target must be an object",
            ));
        }
    };
    let name = vm.format_value(args[1]);
    let original = vm.get_property(target, &name).unwrap_or(Value::Undefined);
    let impl_val = args.get(2).copied().unwrap_or(Value::Undefined);
    let effective = if !matches!(impl_val, Value::Undefined) {
        impl_val
    } else {
        original
    };
    let _ = vm.set_property(target, &name, effective);
    Ok(Value::Undefined)
}

/// `mock.setter(target, name[, impl])`：近似移植（同 getter 限制）。
fn mock_setter(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    mock_getter(vm, args)
}

/// `mock.property(target, name, value)`：以 value 写入属性（读取可观测）。
fn mock_property(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 3 {
        return Err(super::asserts::type_fail(
            vm,
            "mock.property(target, property, value)",
        ));
    }
    let target = match args.first().copied() {
        Some(v) if is_ordinary(vm, v) => v,
        _ => {
            return Err(super::asserts::type_fail(
                vm,
                "mock.property target must be an object",
            ));
        }
    };
    let name = vm.format_value(args[1]);
    let _ = vm.set_property(target, &name, args[2]);
    Ok(Value::Undefined)
}

/// 还原指定槽位（写回 original；独立函数不写回——对齐 Go `restoreAll`）。
pub fn restore_slot(vm: &mut Vm, slot: usize) {
    let Some(state) = take_slot(slot) else {
        return;
    };
    if !state.is_fn {
        if let Some(target) = state.target {
            let _ = vm.set_property(target, &state.method, state.original);
        }
    }
    with_slot_mut(slot, |s| {
        s.impl_val = Value::Undefined;
        s.once_impl = Value::Undefined;
    });
}

/// `mock.restoreAll()`：还原全部并清空（per-test tracker 的还原由
/// 测试结束钩子逐槽位完成，不清理全局槽位表条目以外的状态）。
fn mock_restore_all(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let slots: Vec<usize> = SPY_STORE.with(|s| {
        s.borrow()
            .iter()
            .enumerate()
            .filter(|(_, o)| o.is_some())
            .map(|(i, _)| i)
            .collect()
    });
    for slot in slots {
        restore_slot(vm, slot);
    }
    SPY_STORE.with(|s| {
        s.borrow_mut().clear();
    });
    Ok(Value::Undefined)
}

/// `spy.mock.restore()`：恢复该 spy 的原始实现（calls 历史随 spy 函数
/// 属性保留；槽位清空后 `.mock.calls` 仍可读——对齐 Node 快照语义）。
fn mock_spy_restore(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let mock_ref = match receiver {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let slot = SPY_STORE.with(|s| {
        s.borrow()
            .iter()
            .position(|o| o.as_ref().and_then(|st| st.mock_obj).map(|m| m.0) == Some(mock_ref))
    });
    if let Some(slot) = slot {
        restore_slot(vm, slot);
    }
    Ok(Value::Undefined)
}

/// `mock.reset()`：恢复全部原始实现（调用历史保留——Node 22.23 语义）并复位
/// `mock.timers` 假时钟（Node `MockTracker.reset()` = `restoreAll()` + `timers.reset()`）。
fn mock_reset(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let slots: Vec<usize> = SPY_STORE.with(|s| {
        s.borrow()
            .iter()
            .enumerate()
            .filter(|(_, o)| o.is_some())
            .map(|(i, _)| i)
            .collect()
    });
    for slot in slots {
        restore_slot(vm, slot);
    }
    // Node `MockTracker.reset()` = `restoreAll()` + `timers.reset()`：
    // 同时复位假时钟（还原被接管的全局定时器并清空队列）。
    reset_clock();
    Ok(Value::Undefined)
}

/// 注册 mock 系列处理器（模块 build 时调用一次）。
pub fn register_handlers(registry: &mut crate::builtins::BuiltinRegistry) {
    use crate::builtins::register_handler;
    register_handler(registry, "test:mock", "fn", mock_fn);
    register_handler(registry, "test:mock", "method", mock_method);
    register_handler(registry, "test:mock", "getter", mock_getter);
    register_handler(registry, "test:mock", "setter", mock_setter);
    register_handler(registry, "test:mock", "property", mock_property);
    register_handler(registry, "test:mock", "restoreAll", mock_restore_all);
    register_handler(registry, "test:mock", "reset", mock_reset);
    register_handler(registry, "test:mock", "restoreSpySlot", mock_spy_restore);
    // mock.timers（假时钟）：分派命名空间 test:mockTimers（对象 `_builtinNs`）。
    register_handler(registry, "test:mockTimers", "enable", timers_enable);
    register_handler(registry, "test:mockTimers", "tick", timers_tick);
    register_handler(registry, "test:mockTimers", "setTime", timers_set_time);
    register_handler(registry, "test:mockTimers", "runAll", timers_run_all);
    register_handler(registry, "test:mockTimers", "reset", timers_reset);
}

/// GC 根快照：spy 槽位的 target/原实现/替换实现值。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    SPY_STORE.with(|g| {
        for spy in g.borrow().iter().flatten() {
            if let Some(v) = spy.target {
                out.push(v);
            }
            out.push(spy.original);
            out.push(spy.impl_val);
            out.push(spy.once_impl);
        }
    });
    // 假时钟队列中的回调（`mock.timers` 待执行定时器）。
    FAKE_CLOCK.with(|c| {
        for timer in c.borrow().queue.iter() {
            out.push(timer.cb);
        }
    });
}
