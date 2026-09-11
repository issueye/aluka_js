//! Promise 组合器（`all` / `race` / `allSettled`）与 `.finally`。
//!
//! 语义对齐 Node.js 22 LTS 标准 实测：
//! - `all`：任一拒绝 → 组合器**立即**以该原因拒绝；全部兑现 → 值数组；
//! - `race`：首个定型者胜（兑现或拒绝）；
//! - `allSettled`：永不拒绝，结果为 `{status:"fulfilled",value}` /
//!   `{status:"rejected",reason}` 数组；
//! - `Promise.any` 在 Go 侧不存在（实测 `undefined is not a function`），
//!   为保持对拍一致**不实现**。
//!
//! 机制：组合器状态表按 combiner promise 句柄索引；元素 promise 的定型经
//! `fulfill_promise` / `reject_promise` 出口回调 [`on_settled`] 推进。
//! 组合器的兑现/拒绝经微任务投递解析器（时序与 JS 一致：当前同步段结束后）。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// 组合器种类。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CombinerKind {
    All,
    Race,
    AllSettled,
}

/// 组合器状态。
struct Combiner {
    kind: CombinerKind,
    /// 尚未定型的元素数（Race 仅作展示，胜负由 done 标志决定）
    pending: usize,
    /// 各槽位定型结果 `(value, is_rejected)`
    results: Vec<Option<(Value, bool)>>,
    /// resolve 解析器（成功路径）
    resolver: Value,
    /// reject 解析器（All 首个拒绝 / Race 拒绝胜出）
    reject_resolver: Value,
    /// 已定胜负（Race 首个定型后置位，后续推进忽略）
    done: bool,
}

/// 元素监听条目：(combiner promise 句柄, 槽位)。
type WatchEntry = (u32, usize);

// 组合器表：combiner promise 句柄 → 状态（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static COMBINERS: RefCell<HashMap<u32, Combiner>> = RefCell::new(HashMap::new());
}

// 元素监听表：元素 promise 句柄 → 关联的监听条目列表。
thread_local! {
    static WATCHERS: RefCell<HashMap<u32, Vec<WatchEntry>>> = RefCell::new(HashMap::new());
}

/// 组合器完成动作：经微任务调用对应解析器。
enum Outcome {
    Resolve(Value),
    Reject(Value),
}

impl Vm {
    /// `Promise.all/race/allSettled(iterable)` 公共入口。
    pub(crate) fn promise_combiner(
        &mut self,
        kind: CombinerKind,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let combiner = self.alloc_pending_promise();
        let resolver = self.alloc_promise_resolver(combiner, true);
        let reject_resolver = self.alloc_promise_resolver(combiner, false);
        let elements: Vec<Value> = match args.first().map(|v| v.case()) {
            Some(ValueCase::Object(r)) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::Array { elements, .. }) => elements.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let mut state = Combiner {
            kind,
            pending: elements.len(),
            results: vec![None; elements.len()],
            resolver: Value::Object(resolver),
            reject_resolver: Value::Object(reject_resolver),
            done: false,
        };
        for (slot, el) in elements.iter().enumerate() {
            // 元素定型形态：None = pending promise（登记监听）；Some = 即期结果
            let settled = match el.case() {
                ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                    Some(HeapObject::Promise {
                        pending,
                        value,
                        is_rejected,
                        ..
                    }) => {
                        if *pending {
                            None
                        } else {
                            Some((*value, *is_rejected))
                        }
                    }
                    _ => Some((*el, false)),
                },
                other => Some((other, false)),
            };
            match settled {
                None => WATCHERS.with(|c| {
                    c.borrow_mut()
                        .entry(el_handle(*el))
                        .or_default()
                        .push((combiner.0, slot))
                }),
                Some((value, is_rejected)) => {
                    if let Some(outcome) = record_and_outcome(&mut state, slot, value, is_rejected)
                    {
                        COMBINERS.with(|c| c.borrow_mut().insert(combiner.0, state));
                        self.apply_outcome(combiner, outcome)?;
                        return Ok(Value::Object(combiner));
                    }
                }
            }
        }
        COMBINERS.with(|c| c.borrow_mut().insert(combiner.0, state));
        // 元素全部为即期值：走一次就绪检查（微任务完成组合器）
        self.settle_if_ready(combiner)?;
        Ok(Value::Object(combiner))
    }

    /// `pending == 0` 时构建完成动作并经微任务解析。
    fn settle_if_ready(&mut self, combiner: ObjectRef) -> Result<(), VmError> {
        let Some(outcome) = self.build_ready_outcome(combiner)? else {
            return Ok(());
        };
        self.apply_outcome(combiner, outcome)
    }

    /// 就绪检查 + 完成动作构建（All：拒绝优先；AllSettled：status 对象数组）。
    fn build_ready_outcome(&mut self, combiner: ObjectRef) -> Result<Option<Outcome>, VmError> {
        let snapshot = COMBINERS.with(|c| {
            let mut map = c.borrow_mut();
            let state = map.get_mut(&combiner.0)?;
            if state.done || state.pending > 0 {
                return None;
            }
            state.done = true;
            Some((state.kind, std::mem::take(&mut state.results)))
        });
        let Some((kind, results)) = snapshot else {
            return Ok(None);
        };
        let mut first_rejection: Option<Value> = None;
        let mut values: Vec<Value> = Vec::with_capacity(results.len());
        let mut settled_objs: Vec<Value> = Vec::with_capacity(results.len());
        for r in results.into_iter() {
            let (value, rejected) = r.unwrap_or((Value::Undefined, false));
            if kind == CombinerKind::AllSettled {
                let o = self.alloc_ordinary();
                let status = if rejected { "rejected" } else { "fulfilled" };
                let status_val = self.alloc_string(status.to_owned());
                let _ = self.set_property(Value::Object(o), "status", Value::Object(status_val));
                if rejected {
                    let reason = value;
                    let _ = self.set_property(Value::Object(o), "reason", reason);
                } else {
                    let v = value;
                    let _ = self.set_property(Value::Object(o), "value", v);
                }
                settled_objs.push(Value::Object(o));
            }
            if rejected && first_rejection.is_none() {
                first_rejection = Some(value);
            }
            values.push(if rejected { Value::Undefined } else { value });
        }
        Ok(match kind {
            CombinerKind::All => match first_rejection {
                Some(reason) => Some(Outcome::Reject(reason)),
                None => Some(Outcome::Resolve(Value::Object(self.alloc_array(values)))),
            },
            CombinerKind::AllSettled => Some(Outcome::Resolve(Value::Object(
                self.alloc_array(settled_objs),
            ))),
            CombinerKind::Race => None,
        })
    }

    /// 经微任务把完成动作投递给组合器解析器。
    fn apply_outcome(&mut self, combiner: ObjectRef, outcome: Outcome) -> Result<(), VmError> {
        let (value, resolver) = COMBINERS.with(|c| {
            let map = c.borrow();
            let Some(state) = map.get(&combiner.0) else {
                return (Value::Undefined, Value::Undefined);
            };
            match outcome {
                Outcome::Resolve(v) => (v, state.resolver),
                Outcome::Reject(v) => (v, state.reject_resolver),
            }
        });
        if matches!(resolver, Value::Undefined) {
            return Ok(());
        }
        COMBINERS.with(|c| c.borrow_mut().remove(&combiner.0));
        self.microtask_queue
            .push_back(crate::builtins::Job::Call(resolver, value));
        Ok(())
    }
}

/// 单个元素的即期定型推进：记槽并判定是否即时产生完成动作
/// （All/Race 的拒绝、Race 的首个兑现）。
fn record_and_outcome(
    state: &mut Combiner,
    slot: usize,
    value: Value,
    is_rejected: bool,
) -> Option<Outcome> {
    if state.done {
        return None;
    }
    state.results[slot] = Some((value, is_rejected));
    match state.kind {
        CombinerKind::Race => {
            state.done = true;
            Some(if is_rejected {
                Outcome::Reject(value)
            } else {
                Outcome::Resolve(value)
            })
        }
        CombinerKind::All if is_rejected => {
            // All：首个拒绝立即拒绝组合器（不等待其余元素）
            state.done = true;
            Some(Outcome::Reject(value))
        }
        _ => {
            state.pending = state.pending.saturating_sub(1);
            None
        }
    }
}

/// Promise 定型出口（由 `fulfill_promise` / `reject_promise` 调用）：
/// 推进监听该 promise 的全部组合器。
pub(crate) fn on_settled(
    vm: &mut Vm,
    promise: ObjectRef,
    value: Value,
    is_rejected: bool,
) -> Result<(), VmError> {
    let watchers: Vec<WatchEntry> =
        WATCHERS.with(|c| c.borrow_mut().remove(&promise.0).unwrap_or_default());
    for (combiner_id, slot) in watchers {
        // 即时胜负（Race 胜出 / All 首个拒绝）：直接构建完成动作
        let immediate = COMBINERS.with(|c| {
            let mut map = c.borrow_mut();
            let state = map.get_mut(&combiner_id)?;
            if state.done {
                return None;
            }
            state.results[slot] = Some((value, is_rejected));
            match state.kind {
                CombinerKind::Race => {
                    state.done = true;
                    Some(if is_rejected {
                        Outcome::Reject(value)
                    } else {
                        Outcome::Resolve(value)
                    })
                }
                CombinerKind::All if is_rejected => {
                    state.done = true;
                    Some(Outcome::Reject(value))
                }
                _ => {
                    state.pending = state.pending.saturating_sub(1);
                    None
                }
            }
        });
        let Some(immediate) = immediate else {
            vm.settle_if_ready(ObjectRef(combiner_id))?;
            continue;
        };
        vm.apply_outcome(ObjectRef(combiner_id), immediate)?;
    }
    Ok(())
}

impl Vm {}

/// GC root provider：组合器状态表持有的全部值与解析器。
pub(crate) fn combiner_roots(out: &mut crate::gc::GcRoots) {
    COMBINERS.with(|c| {
        for state in c.borrow().values() {
            if state.done {
                continue;
            }
            out.push(state.resolver);
            out.push(state.reject_resolver);
            for (v, _) in state.results.iter().flatten() {
                out.push(*v);
            }
        }
    });
}

/// 元素 promise 的 Value → 堆句柄（供监听表键控）。
fn el_handle(el: Value) -> u32 {
    match el.case() {
        ValueCase::Object(r) => r.0,
        _ => u32::MAX,
    }
}

/// then/catch/finally 反应：cb 对 + 新 promise 的解析器对。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Reaction {
    /// onFulfilled / onRejected / finally 回调
    pub(crate) on_f: Value,
    pub(crate) on_r: Value,
    /// 新 promise 的 resolve / reject 解析器
    pub(crate) resolver: Value,
    pub(crate) reject_resolver: Value,
}

// then 反应表：promise 句柄 → 待派发反应（pending 期登记，定型时排空）。
thread_local! {
    static THEN_REACTIONS: RefCell<HashMap<u32, Vec<Reaction>>> = RefCell::new(HashMap::new());
}

/// 登记一条反应。
pub(crate) fn push_reaction(target: u32, reaction: Reaction) {
    THEN_REACTIONS.with(|c| c.borrow_mut().entry(target).or_default().push(reaction));
}

/// 排空 promise 的全部反应（定型时调用），返回反应列表。
pub(crate) fn take_reactions(target: u32) -> Vec<Reaction> {
    THEN_REACTIONS.with(|c| c.borrow_mut().remove(&target).unwrap_or_default())
}

/// GC root provider：反应表持有的回调与解析器。
pub(crate) fn reaction_roots(out: &mut crate::gc::GcRoots) {
    THEN_REACTIONS.with(|c| {
        for reactions in c.borrow().values() {
            for r in reactions {
                out.push(r.on_f);
                out.push(r.on_r);
                out.push(r.resolver);
                out.push(r.reject_resolver);
            }
        }
    });
}
