//! 基于指令流前向数据流分析的最大栈深（MaxStack）精确推导器。

use aluka_bytecode::{Instr, Op, TryEntry};
use std::collections::VecDeque;

/// 计算一条指令在给定瞬时上下文中的实际栈深度净变化（delta）。
#[must_use]
pub fn compute_instr_delta(instr: &Instr) -> i32 {
    match instr.op {
        Op::Call => {
            let num_args = instr.operand as i32;
            -num_args
        }
        Op::New => {
            let num_args = instr.operand as i32;
            -num_args
        }
        Op::CallMethod => {
            let num_args = ((instr.operand >> 16) & 0xFFFF) as i32;
            -num_args
        }
        Op::NewObject => {
            let count = instr.operand as i32;
            1 - 2 * count
        }
        Op::NewArray => {
            let count = instr.operand as i32;
            1 - count
        }
        other => other.stack_effect(),
    }
}

/// 计算函数在执行期间可达指令序列中的最大峰值栈深。
#[must_use]
pub fn compute_max_stack(code: &[Instr], try_table: &[TryEntry]) -> u32 {
    if code.is_empty() {
        return 8;
    }

    let n = code.len();
    let mut depths: Vec<Option<i32>> = vec![None; n + 1];
    let mut queue = VecDeque::new();

    depths[0] = Some(0);
    queue.push_back(0);

    // 对于 Try 保护区，如果在异常发生时捕获，Catch 入口栈深度固定为 1（异常对象入栈）
    for entry in try_table {
        if entry.has_catch {
            let catch_idx = (entry.catch_pc / 4) as usize;
            if catch_idx < depths.len() && depths[catch_idx].is_none() {
                depths[catch_idx] = Some(1);
                queue.push_back(catch_idx);
            }
        }
    }

    let mut peak = 0i32;

    while let Some(pc) = queue.pop_front() {
        if pc >= n {
            continue;
        }

        let cur_depth = depths[pc].unwrap_or(0);
        peak = peak.max(cur_depth);

        let instr = &code[pc];
        let delta = compute_instr_delta(instr);
        let next_depth = (cur_depth + delta).max(0);
        peak = peak.max(next_depth);

        // 控制流分支分流
        match instr.op {
            Op::Return | Op::ReturnUndef | Op::Throw => {
                // 终止路径，不往下顺序传播
            }
            Op::Jmp | Op::TryExitJmp => {
                let offset = (instr.operand as i32) / 4;
                let target = (pc as i32 + 1 + offset) as usize;
                if target < depths.len() && depths[target].is_none() {
                    depths[target] = Some(next_depth);
                    queue.push_back(target);
                }
            }
            Op::JmpFalsePop
            | Op::JmpTruePop
            | Op::JmpTrueKeep
            | Op::JmpFalseKeep
            | Op::JmpNullishKeep
            | Op::OptionalJump => {
                // 条件跳转：顺序分支与跳转分支两路传播
                let offset = (instr.operand as i32) / 4;
                let target = (pc as i32 + 1 + offset) as usize;
                if target < depths.len() && depths[target].is_none() {
                    depths[target] = Some(next_depth);
                    queue.push_back(target);
                }
                let seq = pc + 1;
                if seq < depths.len() && depths[seq].is_none() {
                    depths[seq] = Some(next_depth);
                    queue.push_back(seq);
                }
            }
            _ => {
                let seq = pc + 1;
                if seq < depths.len() && depths[seq].is_none() {
                    depths[seq] = Some(next_depth);
                    queue.push_back(seq);
                }
            }
        }
    }

    // 峰值加上适当安全余量（最少为 8，确保调用与构造安全）
    (peak as u32 + 4).max(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_stack_simple_arithmetic() {
        // PushInt, PushInt, Add, Return
        let code = vec![
            Instr::new(Op::PushInt, 10),
            Instr::new(Op::PushInt, 20),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ];
        let max_s = compute_max_stack(&code, &[]);
        assert!(max_s >= 6, "2 个操作数入栈 + 安全余量至少为 6");
    }

    #[test]
    fn test_max_stack_with_conditional_jump() {
        // 0: PushInt 1
        // 1: JmpFalsePop +4 (to 3)
        // 2: PushInt 2
        // 3: Return
        let code = vec![
            Instr::new(Op::PushInt, 1),
            Instr::new(Op::JmpFalsePop, 4),
            Instr::new(Op::PushInt, 2),
            Instr::new(Op::Return, 0),
        ];
        let max_s = compute_max_stack(&code, &[]);
        assert!(max_s >= 8);
    }

    #[test]
    fn test_max_stack_with_try_catch() {
        // 0: PushInt 10
        // 1: Throw
        // 2: ReturnUndef
        let code = vec![
            Instr::new(Op::PushInt, 10),
            Instr::new(Op::Throw, 0),
            Instr::new(Op::ReturnUndef, 0),
        ];
        let try_table = vec![TryEntry {
            start_pc: 0,
            end_pc: 8,
            catch_pc: 8,
            catch_end_pc: 12,
            finally_pc: 0,
            finally_end_pc: 0,
            has_catch: true,
            has_finally: false,
        }];
        let max_s = compute_max_stack(&code, &try_table);
        assert!(max_s >= 8);
    }
}
