//! 异常与 try 语义：throw 传播、handler 查找、try/finally 出口动作。
//!
//! 语义对齐 Go 版 `internal/engine/interpreter/vm_exception.go`（Tier 0 Oracle）：
//! - `THROW` 在当前帧 try 栈自内向外查找 handler；无则包装成
//!   [`VmError::Thrown`] 沿调用链向调用者帧传播；
//! - `RETURN` / `TRY_EXIT_JMP`（break/continue）穿越带 finally 的区域时，
//!   先挂起完成动作（completion）、进入 finally，待 `TRY_EXIT_FINALLY` 恢复；
//! - 已处于 catch/finally 相位的 handler 不再重入，异常直接向外层传播。
//!
//! 注意单位换算：[`aluka_bytecode::TryEntry`] 中的 pc 是**字节偏移**，
//! 而 VM 执行循环的 pc 是**指令索引**（每条指令 4 字节），故查找/跳转前须除以 4。

use crate::interpreter::Vm;
use crate::value::Value;
use aluka_bytecode::TryEntry;

/// handler 相位：正在 try 保护区执行
pub(crate) const PHASE_TRY: u8 = 0;
/// handler 相位：正在 catch 块执行
pub(crate) const PHASE_CATCH: u8 = 1;
/// handler 相位：正在 finally 块执行
pub(crate) const PHASE_FINALLY: u8 = 2;

/// 活跃的 try/catch/finally 区域追踪。
#[derive(Debug)]
pub(crate) struct TryHandler {
    /// 所属 `try_table` 索引（`TRY_EXIT` / `TRY_EXIT_FINALLY` 按此配对）
    pub(crate) try_idx: usize,
    /// 编译期生成的区域描述（pc 为字节偏移）
    pub(crate) entry: TryEntry,
    /// 穿过 finally 的待处理异常（`None` 表示无）
    pub(crate) exc: Option<Value>,
    /// 当前相位（[`PHASE_TRY`] / [`PHASE_CATCH`] / [`PHASE_FINALLY`]）
    pub(crate) phase: u8,
    /// 进入 finally 前挂起的 return/break 完成动作，`TRY_EXIT_FINALLY` 后恢复
    pub(crate) completion: Option<Completion>,
}

/// 穿过 try 区域的完成动作（return 或 break/continue 跳转）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Completion {
    /// 带值返回
    Return(Value),
    /// 跳转到目标指令索引
    Jump(usize),
}

/// `exit_try` 的处理结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TryExitOutcome {
    /// pc 已设置（进入 finally 或完成跳转），执行循环继续
    Continue(usize),
    /// return 已完全解析，调用方直接返回该值
    Return(Value),
}

/// `TRY_EXIT_FINALLY` 的处理结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FinallyOutcome {
    /// 无挂起动作，顺序继续
    Continue,
    /// 恢复挂起的跳转/外层 finally，跳转到目标指令索引
    ContinueAt(usize),
    /// 恢复挂起的异常，重新向外抛出
    Rethrow(Value),
    /// 恢复挂起的 return，直接返回该值
    Return(Value),
}

/// 字节偏移换算为指令索引（每条指令 4 字节）。
#[inline]
fn byte_pc_to_index(bytes: u32) -> usize {
    (bytes / 4) as usize
}

impl Vm {
    /// 在当前帧 try 栈自内向外查找可处理 `exc` 的 handler。
    ///
    /// 命中 catch：压入异常值（catch 代码随后 `StoreLocal` 绑定参数）并跳到
    /// `catch_pc`；命中 finally：跳到 `finally_pc`。相位已 ≥1 的 handler 会被
    /// 弹出跳过（catch/finally 内的异常不再重入本区域）。未命中返回 `None`，
    /// 由调用方把 [`VmError::Thrown`] 向调用者帧传播。
    pub(crate) fn find_handler_in_frame(&mut self, exc: Value) -> Option<usize> {
        let mut i = self.try_stack.len();
        while i > 0 {
            i -= 1;
            if self.try_stack[i].phase >= PHASE_CATCH {
                self.try_stack.truncate(i);
                continue;
            }
            let has_catch = self.try_stack[i].entry.has_catch;
            let has_finally = self.try_stack[i].entry.has_finally;
            if has_catch {
                self.try_stack.truncate(i + 1);
                let catch_pc = byte_pc_to_index(self.try_stack[i].entry.catch_pc);
                let handler = &mut self.try_stack[i];
                handler.phase = PHASE_CATCH;
                handler.exc = Some(exc);
                self.stack.push(exc);
                return Some(catch_pc);
            }
            if has_finally {
                self.try_stack.truncate(i + 1);
                let finally_pc = byte_pc_to_index(self.try_stack[i].entry.finally_pc);
                let handler = &mut self.try_stack[i];
                handler.phase = PHASE_FINALLY;
                handler.exc = Some(exc);
                return Some(finally_pc);
            }
            // 既无 catch 也无 finally：弹出后继续向外查找
            self.try_stack.truncate(i);
        }
        None
    }

    /// 判断跳转目标是否落在 handler 当前相位的活跃区域内（含区域末尾的
    /// `TRY_EXIT` 指令——跳到该指令等价于正常退出，由其自行收尾）。
    fn jump_inside_region(&self, i: usize, target: usize) -> bool {
        let entry = &self.try_stack[i].entry;
        match self.try_stack[i].phase {
            PHASE_TRY => {
                let start = byte_pc_to_index(entry.start_pc);
                let end = byte_pc_to_index(entry.end_pc);
                target >= start && target <= end
            }
            PHASE_CATCH => {
                let start = byte_pc_to_index(entry.catch_pc);
                let end = byte_pc_to_index(entry.catch_end_pc);
                target >= start && target <= end
            }
            _ => {
                let start = byte_pc_to_index(entry.finally_pc);
                let end = byte_pc_to_index(entry.finally_end_pc);
                target >= start && target <= end
            }
        }
    }

    /// 沿当前帧 try 栈处理一次「穿过 try 区域」的完成（return 或跳转）。
    ///
    /// - 跳转目标仍在 handler 活跃区域内 → 保留 handler 直接跳转；
    /// - handler 有 finally 且未进入 → 挂起 completion，进入 finally；
    /// - handler 无 finally → 弹出丢弃（return/break 绕过 catch）;
    /// - 已在 finally（相位 2）→ 丢弃旧 completion 后继续向外。
    pub(crate) fn exit_try(&mut self, completion: Completion) -> TryExitOutcome {
        let mut i = self.try_stack.len();
        while i > 0 {
            i -= 1;
            if let Completion::Jump(target) = completion {
                if self.jump_inside_region(i, target) {
                    self.try_stack.truncate(i + 1);
                    return TryExitOutcome::Continue(target);
                }
            }
            if !self.try_stack[i].entry.has_finally {
                self.try_stack.truncate(i);
                continue;
            }
            if self.try_stack[i].phase <= PHASE_CATCH {
                let finally_pc = byte_pc_to_index(self.try_stack[i].entry.finally_pc);
                let handler = &mut self.try_stack[i];
                handler.completion = Some(completion);
                handler.phase = PHASE_FINALLY;
                self.try_stack.truncate(i + 1);
                return TryExitOutcome::Continue(finally_pc);
            }
            // 相位 2：finally 内的新完成动作覆盖旧的
            self.try_stack[i].completion = None;
            self.try_stack.truncate(i);
        }
        match completion {
            Completion::Jump(pc) => TryExitOutcome::Continue(pc),
            Completion::Return(val) => TryExitOutcome::Return(val),
        }
    }

    /// `TRY_EXIT`：从 try/catch 区域正常退出。
    ///
    /// 相位 1（catch）说明异常已处理，清除挂起异常；有 finally 则转入
    /// finally 相位（handler 保留，由后续 `TRY_EXIT_FINALLY` 弹出），否则弹出。
    pub(crate) fn handle_try_exit(&mut self, try_idx: usize) {
        let mut i = self.try_stack.len();
        while i > 0 {
            i -= 1;
            if self.try_stack[i].try_idx != try_idx {
                continue;
            }
            if self.try_stack[i].phase == PHASE_CATCH {
                self.try_stack[i].exc = None;
            }
            if self.try_stack[i].entry.has_finally {
                self.try_stack[i].phase = PHASE_FINALLY;
            } else {
                self.try_stack.truncate(i);
            }
            return;
        }
    }

    /// `TRY_EXIT_FINALLY`：finally 结束，弹出 handler 并恢复挂起动作。
    pub(crate) fn handle_try_exit_finally(&mut self, try_idx: usize) -> FinallyOutcome {
        let mut i = self.try_stack.len();
        while i > 0 {
            i -= 1;
            if self.try_stack[i].try_idx != try_idx {
                continue;
            }
            // 先取出挂起动作与异常，再弹出 handler（及其上所有残留 handler）
            let (completion, exc) = {
                let handler = &mut self.try_stack[i];
                (handler.completion.take(), handler.exc)
            };
            self.try_stack.truncate(i);
            if let Some(c) = completion {
                // 恢复被 finally 挂起的 return/break：继续向外展开
                // （可能还有外层 finally 待运行）。
                return match self.exit_try(c) {
                    TryExitOutcome::Continue(pc) => FinallyOutcome::ContinueAt(pc),
                    TryExitOutcome::Return(val) => FinallyOutcome::Return(val),
                };
            }
            if let Some(exc) = exc {
                return FinallyOutcome::Rethrow(exc);
            }
            return FinallyOutcome::Continue;
        }
        FinallyOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Vm;
    use aluka_bytecode::TryEntry;

    /// 构造测试用 Try 条目（pc 一律给字节偏移，换算由被测代码负责）。
    fn entry(
        start_pc: u32,
        end_pc: u32,
        catch_pc: u32,
        has_catch: bool,
        finally_pc: u32,
        has_finally: bool,
    ) -> TryEntry {
        TryEntry {
            start_pc,
            end_pc,
            catch_pc,
            has_catch,
            finally_pc,
            has_finally,
            catch_end_pc: if has_catch { end_pc + 40 } else { 0 },
            finally_end_pc: if has_finally { finally_pc + 40 } else { 0 },
        }
    }

    fn handler(try_idx: usize, e: TryEntry) -> TryHandler {
        TryHandler {
            try_idx,
            entry: e,
            exc: None,
            phase: PHASE_TRY,
            completion: None,
        }
    }

    #[test]
    fn throw_with_catch_pushes_exc_and_jumps_to_catch() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 40, 80, true, 120, true)));
        let exc = Value::Number(7.0);
        let next = vm.find_handler_in_frame(exc);
        assert_eq!(next, Some(20), "catch_pc=80 字节应换算为指令 20");
        assert_eq!(
            vm.stack.last(),
            Some(&exc),
            "异常值必须压入栈顶供 StoreLocal 绑定"
        );
        assert_eq!(vm.try_stack[0].phase, PHASE_CATCH);
    }

    #[test]
    fn throw_without_any_handler_propagates() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 40, 0, false, 0, false)));
        assert_eq!(vm.find_handler_in_frame(Value::Null), None);
        assert!(
            vm.try_stack.is_empty(),
            "无 catch/finally 的 handler 应被弹出"
        );
    }

    #[test]
    fn throw_inside_catch_escapes_to_outer_handler() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 40, 200, true, 0, false)));
        // 内层已处于 catch 相位：异常不得重入，须弹出到外层
        let mut inner = handler(1, entry(8, 36, 80, true, 0, false));
        inner.phase = PHASE_CATCH;
        vm.try_stack.push(inner);

        let next = vm.find_handler_in_frame(Value::Number(1.0));
        assert_eq!(next, Some(50), "应命中外层 catch_pc=200 字节 = 指令 50");
        assert_eq!(vm.try_stack.len(), 1, "内层相位>=1 的 handler 必须被弹出");
    }

    #[test]
    fn return_through_finally_suspends_then_resumes() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 40, 0, false, 40, true)));

        let outcome = vm.exit_try(Completion::Return(Value::Number(42.0)));
        assert_eq!(
            outcome,
            TryExitOutcome::Continue(10),
            "应先进入 finally_pc=40 字节"
        );
        assert_eq!(vm.try_stack[0].phase, PHASE_FINALLY);

        // finally 执行完：TRY_EXIT_FINALLY 恢复挂起的 return
        let outcome = vm.handle_try_exit_finally(0);
        assert_eq!(
            outcome,
            FinallyOutcome::Return(Value::Number(42.0)),
            "挂起的 return 值必须原样恢复"
        );
        assert!(vm.try_stack.is_empty());
    }

    #[test]
    fn pending_exception_in_finally_rethrows_on_exit() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 40, 0, false, 40, true)));
        vm.try_stack[0].phase = PHASE_FINALLY;
        vm.try_stack[0].exc = Some(Value::Null);

        let outcome = vm.handle_try_exit_finally(0);
        assert_eq!(outcome, FinallyOutcome::Rethrow(Value::Null));
    }

    #[test]
    fn jump_target_inside_region_keeps_handler() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 100, 0, false, 120, true)));

        let outcome = vm.exit_try(Completion::Jump(20));
        assert_eq!(outcome, TryExitOutcome::Continue(20), "区域内跳转直接执行");
        assert_eq!(
            vm.try_stack.len(),
            1,
            "handler 保留，由区域末尾 TRY_EXIT 收尾"
        );
    }

    #[test]
    fn jump_out_of_region_runs_finally_first() {
        let mut vm = Vm::new(0);
        vm.try_stack
            .push(handler(0, entry(0, 100, 0, false, 120, true)));

        let outcome = vm.exit_try(Completion::Jump(200));
        assert_eq!(
            outcome,
            TryExitOutcome::Continue(30),
            "穿出区域先进入 finally_pc=120 字节"
        );
        let outcome = vm.handle_try_exit_finally(0);
        assert_eq!(
            outcome,
            FinallyOutcome::ContinueAt(200),
            "finally 后恢复 break 跳转"
        );
    }
}
