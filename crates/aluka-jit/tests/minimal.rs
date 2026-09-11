//! 崩溃二分：最小编译单元逐级加复杂度。

use aluka_bytecode::{Constant, FuncTemplate, Instr, Op};
use aluka_jit::jit_compile;
use aluka_vm::Vm;
use std::rc::Rc;

/// 编译 + 经真实 VM 上下文执行（helper 走模块 Import 解析到 VM 的 vtable）。
fn run(vm: &mut Vm, f: &FuncTemplate, args: &[f64]) -> f64 {
    let consts = Rc::new(f.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(f, &ctx.vtable).expect("编译");
    let boxes: Vec<u64> = args
        .iter()
        .map(|&n| aluka_jit::valbox::box_number(n))
        .collect();
    let r = j.call_ctx(&mut ctx, &boxes);
    if aluka_jit::valbox::is_number(r) {
        aluka_jit::valbox::unbox_number(r)
    } else {
        f64::NAN
    }
}

fn func(
    name: &str,
    code: Vec<Instr>,
    consts: Vec<Constant>,
    num_locals: usize,
    params: u32,
) -> FuncTemplate {
    FuncTemplate {
        name: name.to_owned(),
        num_params: params,
        num_locals: num_locals as u32,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack: 16,
        source_file: String::new(),
        constants: consts,
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    }
}

#[test]
fn t1_const_return() {
    let f = func(
        "t1",
        vec![Instr::new(Op::PushConst, 0), Instr::new(Op::Return, 0)],
        vec![Constant::Number(7.5)],
        1,
        0,
    );
    assert_eq!(run(&mut Vm::new(0), &f, &[]), 7.5);
}

#[test]
fn t2_arg_passthrough() {
    let f = func(
        "t2",
        vec![Instr::new(Op::LoadLocal, 1), Instr::new(Op::Return, 0)],
        vec![],
        2,
        1,
    );
    assert_eq!(run(&mut Vm::new(0), &f, &[42.0]), 42.0);
}

#[test]
fn t3_arith() {
    // (arg0 + arg1) * 2
    let f = func(
        "t3",
        vec![
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::LoadLocal, 2),
            Instr::new(Op::Add, 0),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::Mul, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(2.0)],
        3,
        2,
    );
    assert_eq!(run(&mut Vm::new(0), &f, &[3.0, 4.0]), 14.0);
}

#[test]
fn t4_loop_countdown() {
    // n; total=0; while n>0: total+=n; n-=1 → return total  (n=5 → 15)
    let code = vec![
        Instr::new(Op::PushConst, 0), // 0: total=0 (local2)
        Instr::new(Op::StoreLocal, 2),
        // loop_start = 2
        Instr::new(Op::LoadLocal, 1),   // 2: cond n>0
        Instr::new(Op::PushConst, 0),   // 3: 0.0
        Instr::new(Op::Gt, 0),          // 4
        Instr::new(Op::JmpFalsePop, 0), // 5: 假→退出
        Instr::new(Op::LoadLocal, 2),   // 6: total+=n
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::Add, 0),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::LoadLocal, 1), // 11: n-=1
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::StoreLocal, 1),
        // Jmp → 2（回边）
        Instr::new(Op::Jmp, 0),       // 15
        Instr::new(Op::LoadLocal, 2), // 16: exit（JmpFalsePop 真分支落点）
        Instr::new(Op::Return, 0),    // 17
    ];
    // JmpFalsePop(5) 目标 16：signed = 16*4-(5*4+4) = 40 → operand 40
    // Jmp(15) 目标 2：signed = 2*4-(15*4+4) = -56 → 24 位补码
    let mut f = func(
        "t4",
        code,
        vec![Constant::Number(0.0), Constant::Number(1.0)],
        3,
        1,
    );
    // 实际索引：JmpFalsePop 在 5，Jmp 在 14，退出（LoadLocal total）在 15
    let signed_false = (15i32 * 4) - ((5 * 4) + 4);
    f.code[5].operand = (signed_false as i64 & 0xFF_FFFF) as u32;
    let signed_jmp = (2i32 * 4) - ((14 * 4) + 4);
    f.code[14].operand = (signed_jmp as i64 & 0xFF_FFFF) as u32;
    assert_eq!(run(&mut Vm::new(0), &f, &[5.0]), 15.0);
}
