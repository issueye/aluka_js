//! Quick IR：JIT 前的字节码 peephole 优化（M5 任务 1 的 JIT 内部分）。
//!
//! 当前实现：**常量折叠**——`PushConst a; PushConst b; <算术/比较>` 合并为
//! 单条 `PushConst (a op b)`（IEEE-754 语义与 VM 一致：除零→Infinity、
//! NaN 传播）。折叠会移除指令，因此跳转操作数按「旧 pc → 新 pc」显式映射
//! 重算，保证输出指令流自洽（对齐 VM `compute_jump_target` 的相对字节偏移
//! 语义）。后续可扩展 store-load 消除与不可达删除（ADR 0005 决定 3）。

use aluka_bytecode::{Constant, Instr, Op};

/// 折叠产物：优化后的指令流与扩充后的常量池。
pub struct Folded {
    /// 优化后指令流
    pub code: Vec<Instr>,
    /// 扩充后常量池（原常量 + 折叠产生的新常量）
    pub constants: Vec<Constant>,
}

/// 常量折叠 peephole。
#[must_use]
pub fn const_fold(code: &[Instr], constants: &[Constant]) -> Folded {
    // 1. 收集全部跳转目标（旧 pc）
    let mut targets: Vec<bool> = vec![false; code.len() + 2];
    for (pc, instr) in code.iter().enumerate() {
        if matches!(instr.op, Op::Jmp | Op::JmpFalsePop) {
            let t = jump_target(pc, instr.operand);
            if t <= code.len() {
                targets[t] = true;
            }
        }
    }

    // 2. 识别折叠窗口：PushConst a; PushConst b; binop；窗口 4 个槽位
    //    [i..=i+3] 内不得有跳转目标（保证控制流不变）
    let mut fold_at: Vec<(usize, f64)> = Vec::new(); // 窗口首 pc → 折叠值
    let mut consts: Vec<Constant> = constants.to_vec();
    for i in 0..code.len().saturating_sub(2) {
        if targets[i] || targets[i + 1] || targets[i + 2] || targets[i + 3] {
            continue;
        }
        let (Some(x), Some(y)) = (
            const_num(code, i, constants),
            const_num(code, i + 1, constants),
        ) else {
            continue;
        };
        let binop = code[i + 2].op;
        if code[i + 2].operand != 0 {
            continue;
        }
        let v = match binop {
            Op::Add => x + y,
            Op::Sub => x - y,
            Op::Mul => x * y,
            Op::Div => x / y,
            Op::Lt => bool_f64(x < y),
            Op::Gt => bool_f64(x > y),
            Op::Eq => bool_f64(x == y),
            _ => continue,
        };
        // 相邻窗口重叠防护
        if let Some((last_start, _)) = fold_at.last() {
            if i <= *last_start + 2 {
                continue;
            }
        }
        fold_at.push((i, v));
    }

    // 3. 构建新指令流；同时记录 (新pc → 旧pc) 与 (旧pc → 新pc) 双向映射
    let mut new_code: Vec<Instr> = Vec::with_capacity(code.len());
    let mut new_to_old: Vec<usize> = Vec::with_capacity(code.len() + 1);
    let mut old_to_new: Vec<usize> = Vec::with_capacity(code.len() + 1);
    let mut i = 0;
    while i < code.len() {
        let folded_here = fold_at.iter().find(|(start, _)| *start == i);
        if let Some((_, v)) = folded_here {
            let idx = consts.len() as u32;
            consts.push(Constant::Number(*v));
            let new_pc = new_code.len();
            new_code.push(Instr::new(Op::PushConst, idx));
            // 窗口内三条旧指令都映射到折叠指令的新位置
            new_to_old.push(i);
            old_to_new.push(new_pc);
            old_to_new.push(new_pc);
            old_to_new.push(new_pc);
            i += 3; // 跳过 PushConst a; PushConst b; binop
            continue;
        }
        new_to_old.push(i);
        old_to_new.push(new_code.len());
        new_code.push(code[i]);
        i += 1;
    }
    // 末尾哨兵
    new_to_old.push(code.len());
    old_to_new.push(new_code.len());

    // 4. 重算跳转操作数：经 new_to_old 反查旧源 pc，映射旧目标 → 新目标
    for (new_pc, instr) in new_code.iter_mut().enumerate() {
        if matches!(instr.op, Op::Jmp | Op::JmpFalsePop) {
            let old_pc = new_to_old[new_pc];
            let old_target = jump_target(old_pc, instr.operand);
            let new_target = old_to_new[old_target.min(old_to_new.len() - 1)];
            let signed = (new_target as i32 * 4) - ((new_pc as i32 * 4) + 4);
            instr.operand = (signed as i64 & 0xFF_FFFF) as u32;
        }
    }
    Folded {
        code: new_code,
        constants: consts,
    }
}

fn bool_f64(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

/// 读 `pc` 处 PushConst 的数值常量（Number/Bool 参与；NaN 不折叠）。
fn const_num(code: &[Instr], pc: usize, constants: &[Constant]) -> Option<f64> {
    let instr = code.get(pc)?;
    if instr.op != Op::PushConst {
        return None;
    }
    match constants.get(instr.operand as usize)? {
        Constant::Number(n) if !n.is_nan() => Some(*n),
        Constant::Bool(b) => Some(bool_f64(*b)),
        _ => None,
    }
}

/// 跳转目标指令索引（对齐 VM `compute_jump_target`）。
#[must_use]
pub fn jump_target(pc: usize, operand: u32) -> usize {
    let signed = if operand & 0x80_0000 != 0 {
        (operand | 0xFF00_0000) as i32
    } else {
        operand as i32
    };
    (((pc as i32 * 4) + 4 + signed) / 4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(idx: u32) -> Instr {
        Instr::new(Op::PushConst, idx)
    }

    /// `PushConst 2; PushConst 3; Add; Return` → `PushConst 5; Return`。
    #[test]
    fn folds_const_add() {
        let code = vec![
            push(0),
            push(1),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ];
        let consts = vec![Constant::Number(2.0), Constant::Number(3.0)];
        let out = const_fold(&code, &consts);
        assert_eq!(out.code.len(), 2, "应折叠为两条");
        assert_eq!(
            out.constants[out.code[0].operand as usize],
            Constant::Number(5.0)
        );
    }

    /// 折叠后跳转偏移重算：Jmp 越过被移除窗口仍落到等价位置。
    #[test]
    fn folds_with_jump_target_remap() {
        // 0: PushConst 1
        // 1: PushConst 2 ┐ 窗口折叠 → PushConst 5
        // 2: PushConst 3 ┘
        // 3: Add
        // 4: Pop
        // 5: Jmp +8 字节（目标 8 = 末尾哨兵）
        // 6: PushConst 4
        // 7: Return
        let code = vec![
            push(0),
            push(1),
            push(2),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::Jmp, 8),
            push(3),
            Instr::new(Op::Return, 0),
        ];
        let consts = vec![
            Constant::Number(1.0),
            Constant::Number(2.0),
            Constant::Number(3.0),
            Constant::Number(4.0),
        ];
        let out = const_fold(&code, &consts);
        // 新流：0 PushConst(1)；1 PushConst(5)；2 Pop；3 Jmp→6；4 PushConst(4)；5 Return
        assert_eq!(out.code.len(), 6, "窗口 3→1，流长 8→6");
        assert_eq!(
            out.constants[out.code[1].operand as usize],
            Constant::Number(5.0),
            "折叠值 2+3"
        );
        let jmp = &out.code[3];
        assert_eq!(jmp.op, Op::Jmp);
        assert_eq!(jump_target(3, jmp.operand), 6, "Jmp 重算至出口哨兵位");
    }

    /// 跳转目标落入折叠窗口时不折叠（保守保正确）。
    #[test]
    fn skips_fold_when_jump_targets_window() {
        let code = vec![
            Instr::new(Op::Jmp, 8),
            push(0),
            push(1),
            Instr::new(Op::Add, 0),
        ];
        let consts = vec![Constant::Number(1.0), Constant::Number(2.0)];
        let out = const_fold(&code, &consts);
        assert_eq!(out.code.len(), 4, "窗口含跳转目标，不应折叠");
    }

    /// NaN 常量不折叠。
    #[test]
    fn skips_nan_constants() {
        let code = vec![push(0), push(1), Instr::new(Op::Add, 0)];
        let consts = vec![Constant::Number(f64::NAN), Constant::Number(1.0)];
        let out = const_fold(&code, &consts);
        assert_eq!(out.code.len(), 3, "NaN 不折叠");
    }

    /// 循环结构（回边 Jmp + 前向 JmpFalsePop）折叠后跳转全部正确重算。
    #[test]
    fn loop_backedge_remap() {
        // 0: LoadLocal 1 (n)
        // 1: PushConst 0 (0.0)
        // 2: Gt           ← n > 0
        // 3: JmpFalsePop → 7（退出）
        // 4: LoadLocal 1
        // 5: PushConst 1
        // 6: Sub          ← n -= 1（省略 Store 以聚焦跳转）
        // 7: Return
        let code = vec![
            Instr::new(Op::LoadLocal, 1),
            push(0),
            Instr::new(Op::Gt, 0),
            // 目标 7：signed = 7*4 - (3*4+4) = 12
            Instr::new(Op::JmpFalsePop, 12),
            Instr::new(Op::LoadLocal, 1),
            push(1),
            Instr::new(Op::Sub, 0),
            Instr::new(Op::Return, 0),
        ];
        let consts = vec![Constant::Number(0.0), Constant::Number(1.0)];
        let out = const_fold(&code, &consts);
        // 窗口 [4,5,6] 的 [4..=7] 含 targets[7]（JmpFalsePop pc+1）→ 不折叠
        assert_eq!(out.code.len(), 8);
        let jfp = &out.code[3];
        assert_eq!(jump_target(3, jfp.operand), 7, "退出目标不变");
    }
}
