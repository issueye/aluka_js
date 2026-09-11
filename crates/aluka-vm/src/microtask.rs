//! 微任务与宏任务调度（M2）：nextTick 优先队列 + Promise 回调 + 定时器。
//!
//! Node 语义对齐：`process.nextTick` 队列优先于 Promise 微任务队列，全部
//! 微任务清空后才执行宏任务（`setTimeout`）。当前为**线性驱动**模型——
//! 顶层执行结束后一次性排空两段队列（05-es2024 等用例的同步 fulfill 模式
//! 足够）；真正的异步挂起（await pending Promise 的中途恢复）留后续。

use crate::generator::SuspendedFrame;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// 微任务载荷：普通回调（携带兑现值）或挂起 async 帧的恢复任务。
#[derive(Debug)]
pub(crate) enum Job {
    /// 调用回调（this=undefined，实参 [arg]）
    Call(Value, Value),
    /// 恢复挂起的 async 帧（await 的 promise 已兑现）
    ResumeFrame(PendingResume),
    /// 恢复挂起的 async 帧（await 的 promise 已拒绝：进帧即抛，命中帧内 try/catch）
    ResumeFrameRejected(PendingResume),
    /// then/finally 反应：回调定型值/原因，结果采纳进新 promise
    /// （`is_finally`：回调结果被忽略，原值/原因透传）。
    Reaction {
        cb: Value,
        arg: Value,
        resolver: Value,
        reject_resolver: Value,
        is_finally: bool,
    },
    /// 透传第二跳：先排空当轮，再入队解析器调用（对齐 Node.js 22 LTS 标准 透传时序——
    /// 已定型 promise 的缺失回调透传比常规反应晚一个微任务）
    ResolveLater {
        resolver: Value,
        arg: Value,
    },
    RejectLater {
        resolver: Value,
        arg: Value,
    },
}

/// 挂起 async 帧的恢复登记。
#[derive(Debug)]
pub(crate) struct PendingResume {
    /// 挂起帧快照
    pub frame: SuspendedFrame,
    /// 所属函数模板索引
    pub func_idx: usize,
    /// 本 async 函数的 promise（帧完成时兑现；被 await 则级联恢复调用者）
    pub promise: ObjectRef,
    /// 被 await 的 promise（兑现值即恢复后 AWAIT 的结果）
    pub awaited: ObjectRef,
}

impl Vm {
    /// 恢复挂起的 async 帧：换入帧上下文，从挂起 pc 继续执行至完成/再挂起。
    ///
    /// `rejected` 为 `true` 时表示 await 的 promise 已拒绝：不注入兑现值，而是
    /// 在帧内查找 try/catch——命中则从 handler 入口续跑（catch 绑定拒绝原因）；
    /// 未命中则本 async 函数的 promise 级联拒绝（JS 语义）。
    ///
    /// 完成时兑现该 async 函数的 promise（若被 await 则级联恢复调用者帧）。
    fn resume_async_frame(
        &mut self,
        frame: crate::builtins::PendingResume,
        rejected: bool,
    ) -> Result<(), VmError> {
        let caller = crate::generator::CallerFrame::save(self);
        let base = self.stack.len();
        let (code, constants, try_table) = {
            let tmpl = &self.module_functions[frame.func_idx];
            (
                tmpl.code.clone(),
                self.module_constants[frame.func_idx].clone(),
                tmpl.try_table.clone(),
            )
        };
        let SuspendedFrame {
            pc,
            locals,
            stack,
            upvalues,
            open_upvalues,
            try_stack,
        } = frame.frame;
        self.locals = locals;
        self.current_upvalues = upvalues;
        self.open_upvalues = open_upvalues;
        self.try_stack = try_stack;
        self.stack.extend(stack);
        // AWAIT 的兑现值：挂起时已把 operand 弹出，恢复时压回（挂起指令的
        // 语义补全——被 await 的 promise 的当前值即恢复后的 AWAIT 结果）
        let resumed_value = match self.heap.get(frame.awaited.index()) {
            Some(HeapObject::Promise { value, .. }) => *value,
            _ => Value::Undefined,
        };
        self.current_try_table = try_table;
        self.current_func_idx = frame.func_idx as i64;

        // 拒绝恢复：不压兑现值，先在帧内找 handler（find_handler_in_frame 会把
        // 拒绝原因压栈供 catch 参数绑定）；帧内无 handler 则级联拒绝本 promise。
        let entry_pc = if rejected {
            match self.find_handler_in_frame(resumed_value) {
                Some(catch_pc) => catch_pc,
                None => {
                    caller.restore(self);
                    return self.reject_promise(frame.promise, resumed_value);
                }
            }
        } else {
            self.stack.push(resumed_value);
            pc
        };

        let outcome = self.run_with_constants_rc(&code, constants, entry_pc);

        // 收割 async 帧（与 drive_generator 相同的收割/恢复模式）
        let gen_stack = self.stack.split_off(base);
        let gen_locals = std::mem::take(&mut self.locals);
        let gen_upvalues = std::mem::take(&mut self.current_upvalues);
        let gen_open_upvalues = std::mem::take(&mut self.open_upvalues);
        let gen_try_stack = std::mem::take(&mut self.try_stack);
        caller.restore(self);

        match outcome {
            Ok(ret) => {
                // async 函数完成：兑现其 promise（若被 await 则级联恢复调用者）
                self.fulfill_promise(frame.promise, ret)?;
            }
            Err(VmError::Awaited(promise)) => {
                // 再次挂起：登记新恢复帧（完成时兑现本 async 的 promise）
                self.promise_resumes.insert(
                    promise.index() as u32,
                    crate::builtins::PendingResume {
                        frame: crate::generator::SuspendedFrame {
                            pc: self.yield_pc,
                            locals: gen_locals,
                            stack: gen_stack,
                            upvalues: gen_upvalues,
                            open_upvalues: gen_open_upvalues,
                            try_stack: gen_try_stack,
                        },
                        func_idx: frame.func_idx,
                        promise: frame.promise,
                        awaited: promise,
                    },
                );
            }
            Err(VmError::Thrown(exc)) => {
                // async 函数体内未捕获异常：其 promise 以该异常拒绝（JS 语义，
                // 不向顶层传播——拒绝沿 .catch/await 链继续传递；caller 已在
                // 上方收割时恢复，无需再次还原）
                return self.reject_promise(frame.promise, exc);
            }
            Err(err) => {
                return Err(err);
            }
        }
        Ok(())
    }

    /// 排空微任务：nextTick 队列全部执行后，Promise 回调队列循环执行
    /// （执行中可能追加新回调）。
    pub(crate) fn drain_microtasks(&mut self) -> Result<(), VmError> {
        // 1. nextTick 优先（整段清空）
        while let Some(cb) = self.nexttick_queue.pop_front() {
            self.invoke_callable(cb, Value::Undefined, &[])?;
        }
        // 2. Promise 回调循环（新增回调在本轮继续执行；回调携带兑现值；
        //    Resume 帧恢复后继续 async 函数）
        while let Some(job) = self.microtask_queue.pop_front() {
            match job {
                Job::Call(cb, arg) => {
                    self.invoke_callable(cb, Value::Undefined, &[arg])?;
                }
                Job::ResumeFrame(resume) => {
                    self.resume_async_frame(resume, false)?;
                }
                Job::ResumeFrameRejected(resume) => {
                    self.resume_async_frame(resume, true)?;
                }
                Job::Reaction {
                    cb,
                    arg,
                    resolver,
                    reject_resolver,
                    is_finally,
                } => {
                    match self.invoke_callable(cb, Value::Undefined, &[arg]) {
                        Err(VmError::Thrown(exc)) => {
                            // 回调抛错：新 promise 以异常拒绝
                            self.microtask_queue
                                .push_back(Job::Call(reject_resolver, exc));
                        }
                        Ok(ret) => {
                            if is_finally {
                                // finally：忽略回调返回值，原定型值透传
                                self.microtask_queue.push_back(Job::Call(resolver, arg));
                            } else {
                                self.adopt_and_resolve(ret, resolver, reject_resolver)?;
                            }
                        }
                        Err(err) => return Err(err),
                    }
                }
                Job::ResolveLater { resolver, arg } => {
                    self.microtask_queue.push_back(Job::Call(resolver, arg));
                }
                Job::RejectLater { resolver, arg } => {
                    self.microtask_queue.push_back(Job::Call(resolver, arg));
                }
            }
        }
        Ok(())
    }

    /// then 回调返回值的采纳（JS Promise AdoptState）：返回 pending promise
    /// 则挂接新 promise 的解析器对随其定型；否则直接以返回值兑现/拒绝。
    fn adopt_and_resolve(
        &mut self,
        ret: Value,
        resolver: Value,
        reject_resolver: Value,
    ) -> Result<(), VmError> {
        let adopted = match ret.case() {
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::Promise {
                    pending,
                    value,
                    is_rejected,
                    ..
                }) => {
                    if *pending {
                        // 挂接：ret 兑现 → resolver(即兑现 P2)；ret 拒绝 → reject_resolver
                        if let Some(HeapObject::Promise {
                            handlers, rejected, ..
                        }) = self.heap.get_mut(r.0 as usize)
                        {
                            handlers.push(resolver);
                            rejected.push(reject_resolver);
                        }
                        // 写屏障：老 promise 挂接年轻解析器（adoption）
                        self.gc_write_barrier(r, resolver);
                        self.gc_write_barrier(r, reject_resolver);
                        None
                    } else if *is_rejected {
                        Some((reject_resolver, *value))
                    } else {
                        Some((resolver, *value))
                    }
                }
                _ => Some((resolver, ret)),
            },
            other => Some((resolver, other)),
        };
        if let Some((resolver, value)) = adopted {
            self.microtask_queue.push_back(Job::Call(resolver, value));
        }
        Ok(())
    }

    /// 排空宏任务（`setTimeout`/`setInterval`）：按**到期时间**升序执行
    /// （同批注册的定时器按注册顺序，周期任务到期后重排 `due += delay`）。
    /// 已 clear 的句柄跳过。末尾泵一轮内置库事件源（net/http/child_process
    /// 等注册的 I/O 泵），返回「本轮是否执行了定时器或事件泵有进展」——
    /// 顶层事件循环据此决定是否继续交替排空。
    pub(crate) fn drain_macro_tasks(&mut self) -> Result<bool, VmError> {
        // 收集全部任务，反复取「到期最早」的执行（用例规模小，线性扫描足够）
        let mut tasks: Vec<(u64, u64, u64, Value, bool)> = self.macro_tasks.drain(..).collect();
        let mut now = 0u64;
        let mut ran_timer = false;
        loop {
            let mut best: Option<(usize, u64)> = None;
            for (i, (_, due, _, _, _)) in tasks.iter().enumerate() {
                if best.is_none_or(|(_, bd)| *due < bd) {
                    best = Some((i, *due));
                }
            }
            let Some((idx, due)) = best else { break };
            let (id, _, delay_ms, cb, repeating) = tasks.remove(idx);
            if due > now {
                self.wait_until_due(due, &mut now)?;
            }
            if self.active_timers.contains(&id) {
                continue;
            }
            self.invoke_callable(cb, Value::Undefined, &[])?;
            ran_timer = true;
            if repeating && !self.active_timers.contains(&id) {
                tasks.push((id, due + delay_ms, delay_ms, cb, true));
            }
        }
        // 内置库事件源泵：宏任务排空后轮询 I/O 事件源，有进展则告知调用方
        // 继续交替排空（事件回调可能追加微任务 / 宏任务）。
        let pumped = self.pump_event_sources()?;
        Ok(ran_timer || pumped)
    }

    /// 把虚拟时钟推进到 `due`，期间**不得饿死事件源**。
    ///
    /// 有活跃事件源时必须分片等待、片间泵事件源：Node 中定时器与 I/O 同属
    /// 一个轮询循环，而在定时器等待中整段 `sleep` 会让「待触发定时器」把
    /// worker 消息、网络回包等事件**全部推迟到定时器触发之后**——实测缺陷：
    /// `setTimeout(fn, 2000)` 期间 worker 回包在 2000ms 后才被投递；若定时器
    /// 回调里 `terminate()`，消息则永久丢失（Node 两种情形都立即投递）。
    ///
    /// 无活跃事件源时保持原语义一次睡满，避免长定时器空转吃 CPU。
    fn wait_until_due(&mut self, due: u64, now: &mut u64) -> Result<(), VmError> {
        /// 分片粒度（ms）：越小消息投递越及时，越大空转越少。
        const WAIT_SLICE_MS: u64 = 1;
        if !self.has_active_event_sources() {
            std::thread::sleep(std::time::Duration::from_millis(due - *now));
            *now = due;
            return Ok(());
        }
        while *now < due {
            let slice = (due - *now).min(WAIT_SLICE_MS);
            std::thread::sleep(std::time::Duration::from_millis(slice));
            *now += slice;
            // 泵出的回调可能追加微任务/宏任务；新增宏任务落在 self.macro_tasks，
            // 由顶层循环下一轮接手（本轮 tasks 已在本地，不重入）。
            self.pump_event_sources()?;
        }
        Ok(())
    }

    /// Promise 兑现：设定值与处理器，把全部处理器调度进微任务队列。
    ///
    /// 已定型的 promise（fulfilled/rejected）再次 resolve/reject 为 no-op（JS 语义）。
    pub(crate) fn fulfill_promise(
        &mut self,
        promise: aluka_core::ObjectRef,
        value: Value,
    ) -> Result<(), VmError> {
        let handlers = {
            let Some(HeapObject::Promise {
                pending,
                value: slot,
                is_rejected,
                handlers,
                ..
            }) = self.heap.get_mut(promise.index())
            else {
                return Ok(());
            };
            if !*pending {
                return Ok(());
            }
            *pending = false;
            *is_rejected = false;
            *slot = value;
            std::mem::take(handlers)
        };
        for handler in handlers {
            self.microtask_queue
                .push_back(crate::builtins::Job::Call(handler, value));
        }
        // 挂起的 async 帧等待本 promise 兑现：排入恢复任务
        if let Some(resume) = self.promise_resumes.remove(&promise.0) {
            self.microtask_queue
                .push_back(crate::builtins::Job::ResumeFrame(resume));
        }
        // then 反应排空：onF 派发 / 缺失则兑现透传
        for reaction in crate::builtins::promise::take_reactions(promise.0) {
            if !matches!(reaction.on_f, Value::Undefined) {
                self.microtask_queue.push_back(Job::Reaction {
                    cb: reaction.on_f,
                    arg: value,
                    resolver: reaction.resolver,
                    reject_resolver: reaction.reject_resolver,
                    is_finally: false,
                });
            } else {
                self.microtask_queue
                    .push_back(Job::Call(reaction.resolver, value));
            }
        }
        // 组合器（all/race/allSettled）推进：元素定型出口
        crate::builtins::promise::on_settled(self, promise, value, false)?;
        Ok(())
    }

    /// Promise 拒绝：设定拒绝原因，把全部 onRejected 处理器调度进微任务队列；
    /// 挂起的 async 帧等待本 promise 则以「进帧即抛」恢复（rejection 语义）。
    /// 已定型的 promise 再次 reject/resolve 为 no-op（JS 语义）。
    pub(crate) fn reject_promise(
        &mut self,
        promise: aluka_core::ObjectRef,
        value: Value,
    ) -> Result<(), VmError> {
        let handlers = {
            let Some(HeapObject::Promise {
                pending,
                value: slot,
                is_rejected,
                rejected,
                ..
            }) = self.heap.get_mut(promise.index())
            else {
                return Ok(());
            };
            if !*pending {
                return Ok(());
            }
            *pending = false;
            *is_rejected = true;
            *slot = value;
            std::mem::take(rejected)
        };
        for handler in handlers {
            self.microtask_queue
                .push_back(crate::builtins::Job::Call(handler, value));
        }
        if let Some(resume) = self.promise_resumes.remove(&promise.0) {
            self.microtask_queue
                .push_back(crate::builtins::Job::ResumeFrameRejected(resume));
        }
        // then 反应排空：onR 派发 / 缺失则拒绝透传
        for reaction in crate::builtins::promise::take_reactions(promise.0) {
            if !matches!(reaction.on_r, Value::Undefined) {
                self.microtask_queue.push_back(Job::Reaction {
                    cb: reaction.on_r,
                    arg: value,
                    resolver: reaction.resolver,
                    reject_resolver: reaction.reject_resolver,
                    is_finally: false,
                });
            } else {
                self.microtask_queue
                    .push_back(Job::Call(reaction.reject_resolver, value));
            }
        }
        // 组合器推进（All 首个拒绝 / Race 拒绝胜出 / AllSettled 记槽）
        crate::builtins::promise::on_settled(self, promise, value, true)?;
        Ok(())
    }
}
