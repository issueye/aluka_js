//! M5 jitdiff：生成式差分——同一子集字节码在解释器与 JIT 上零失配。
//!
//! 生成器构造随机算术/条件/有界循环函数，三路验证：
//! 1. `BytecodeModule::verify`（静态 ISA 规范）；
//! 2. aluvm 解释器执行（`Vm::run_func`）；
//! 3. `aluka-jit` 机器码执行。
//!
//! 解释器与 JIT 的 f64 结果**逐位相等**（含 NaN/Infinity 位型一致）。
//!
//! 生成的函数不读实参（解释器经帧槽绑定、JIT 经指针数组，参数约定不同），
//! 随机程序以常量初始化局部，保证两侧执行同一份纯数值计算。

use aluka_bytecode::{BytecodeModule, Constant, FuncTemplate, Instr, Op};
use aluka_jit::jit_compile;
use aluka_vm::Value;

/// 简易确定性 RNG（xorshift64，可复现）。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// JIT 盒 → f64（Number 出盒；TRUE/FALSE → 1/0；其余 NaN）。
fn box_to_f64(b: u64) -> f64 {
    if aluka_jit::valbox::is_number(b) {
        aluka_jit::valbox::unbox_number(b)
    } else if b == aluka_jit::valbox::TRUE {
        1.0
    } else if b == aluka_jit::valbox::FALSE {
        0.0
    } else {
        f64::NAN
    }
}

/// 数值化 VM 结果（Boolean → 1.0/0.0，与 JIT 表示对齐）。
fn to_f64(v: Value) -> f64 {
    match v.case() {
        aluka_vm::value::ValueCase::Number(n) => n,
        aluka_vm::value::ValueCase::Boolean(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        _ => f64::NAN,
    }
}

/// 属性访问子集用例（约 1/4）：o.a=v1; o.b=v2; 尾表达式 o.a+o.b+o.a
/// （同名站点二次读取触发 PIC 命中）；偶发读缺失属性触发 helper 回退 → NaN。
fn generate_props(rng: &mut Rng, id: usize) -> FuncTemplate {
    let a_idx = 0u32; // String("a")
    let b_idx = 1u32; // String("b")
    let mut consts: Vec<Constant> = vec![
        Constant::String("a".to_owned()),
        Constant::String("b".to_owned()),
    ];
    let v1 = consts.len() as u32;
    consts.push(Constant::Number(1.0 + rng.below(100) as f64));
    let v2 = consts.len() as u32;
    consts.push(Constant::Number(1.0 + rng.below(100) as f64));
    let c_name = consts.len() as u32;
    consts.push(Constant::String("c".to_owned())); // 未设置的属性名

    let mut code: Vec<Instr> = vec![Instr::new(Op::NewObject, 0), Instr::new(Op::StoreLocal, 1)];
    // o.a = v1；o.b = v2（先压 obj 再压 val：SetProp 弹 val、obj）
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::PushConst, v1));
    code.push(Instr::new(Op::SetProp, a_idx));
    code.push(Instr::new(Op::Pop, 0));
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::PushConst, v2));
    code.push(Instr::new(Op::SetProp, b_idx));
    code.push(Instr::new(Op::Pop, 0));
    // o.a + o.b + o.a（第二次读 o.a 命中同一 PIC 站点）
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::GetProp, a_idx));
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::GetProp, b_idx));
    code.push(Instr::new(Op::Add, 0));
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::GetProp, a_idx));
    code.push(Instr::new(Op::Add, 0));
    // 偶发缺属性：o.c 缺失 → undefined → NaN（helper 回退路径）
    // 偶发缺属性：o.c 缺失 → undefined → NaN；乘到当前值上作为尾返回
    if rng.below(3) == 0 {
        code.push(Instr::new(Op::LoadLocal, 1));
        code.push(Instr::new(Op::GetProp, c_name));
        code.push(Instr::new(Op::Mul, 0));
    }
    code.push(Instr::new(Op::Return, 0));

    FuncTemplate {
        name: format!("jitdiff_props_{id}"),
        num_params: 0,
        num_locals: 2,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack: 64,
        source_file: format!("gen_{id}"),
        constants: consts,
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    }
}

/// 生成随机子集函数：常量初始化局部 → 有界 countdown 循环 → 尾表达式。
fn generate(rng: &mut Rng, id: usize) -> FuncTemplate {
    // 约 1/4 用例走属性访问（PIC 差分）
    if rng.below(4) == 0 {
        return generate_props(rng, id);
    }
    let mut consts: Vec<Constant> = vec![
        Constant::Number(0.0), // 0
        Constant::Number(1.0), // 1
        Constant::Number(2.0), // 2
    ];
    let mut code: Vec<Instr> = Vec::new();
    let n_slot = 1u32;
    let acc: [u32; 3] = [2, 3, 4];

    // n = 1..=12（有界终止）
    let n0 = 1.0 + rng.below(12) as f64;
    let idx_n0 = consts.len() as u32;
    consts.push(Constant::Number(n0));
    code.push(Instr::new(Op::PushConst, idx_n0));
    code.push(Instr::new(Op::StoreLocal, n_slot));

    for &slot in &acc {
        let v = (rng.below(50) as f64) - 20.0 + (rng.below(4) as f64) / 4.0;
        let idx = consts.len() as u32;
        consts.push(Constant::Number(v));
        code.push(Instr::new(Op::PushConst, idx));
        code.push(Instr::new(Op::StoreLocal, slot));
    }

    // while n > 0 { acc0 = acc0 op1 acc1; acc1 = acc1 op2 k; n -= 1 }
    let loop_start = code.len();
    code.push(Instr::new(Op::LoadLocal, n_slot));
    code.push(Instr::new(Op::PushConst, 0));
    code.push(Instr::new(Op::Gt, 0));
    let jfp_at = code.len();
    code.push(Instr::new(Op::JmpFalsePop, 0));

    let op1 = match rng.below(4) {
        0 => Op::Add,
        1 => Op::Sub,
        2 => Op::Mul,
        _ => Op::Div,
    };
    code.push(Instr::new(Op::LoadLocal, acc[0]));
    code.push(Instr::new(Op::LoadLocal, acc[1]));
    code.push(Instr::new(op1, 0));
    code.push(Instr::new(Op::StoreLocal, acc[0]));

    let op2 = match rng.below(4) {
        0 => Op::Add,
        1 => Op::Sub,
        2 => Op::Mul,
        _ => Op::Div,
    };
    let k = 1.0 + (rng.below(6) as f64) / 2.0;
    let idx_k = consts.len() as u32;
    consts.push(Constant::Number(k));
    code.push(Instr::new(Op::LoadLocal, acc[1]));
    code.push(Instr::new(Op::PushConst, idx_k));
    code.push(Instr::new(op2, 0));
    code.push(Instr::new(Op::StoreLocal, acc[1]));

    code.push(Instr::new(Op::LoadLocal, n_slot));
    code.push(Instr::new(Op::PushConst, 1));
    code.push(Instr::new(Op::Sub, 0));
    code.push(Instr::new(Op::StoreLocal, n_slot));

    let jmp_at = code.len();
    code.push(Instr::new(Op::Jmp, 0));
    let exit_pc = code.len();

    // 尾表达式：随机组合累加器，可选比较收尾
    let tail_op = match rng.below(3) {
        0 => Op::Add,
        1 => Op::Sub,
        _ => Op::Mul,
    };
    code.push(Instr::new(Op::LoadLocal, acc[0]));
    code.push(Instr::new(Op::LoadLocal, acc[1]));
    code.push(Instr::new(tail_op, 0));
    if rng.below(2) == 0 {
        code.push(Instr::new(Op::LoadLocal, acc[2]));
        code.push(Instr::new(Op::Add, 0));
    }
    if rng.below(3) == 0 {
        code.push(Instr::new(Op::PushConst, 2));
        let cmp = match rng.below(3) {
            0 => Op::Lt,
            1 => Op::Gt,
            _ => Op::Eq,
        };
        code.push(Instr::new(cmp, 0));
    }
    code.push(Instr::new(Op::Return, 0));

    // 回填跳转（相对下一指令的字节偏移）
    let patch = |code: &mut Vec<Instr>, at: usize, target: usize| {
        let signed = (target as i32 * 4) - ((at as i32 * 4) + 4);
        code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
    };
    patch(&mut code, jfp_at, exit_pc);
    patch(&mut code, jmp_at, loop_start);

    FuncTemplate {
        name: format!("jitdiff_{id}"),
        num_params: 0,
        num_locals: 5,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack: 64,
        source_file: format!("gen_{id}"),
        constants: consts,
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    }
}

/// 解释器执行（复用调用方传入的 VM：内置原型/构造器单例只需预建一次）。
fn interp_run(vm: &mut aluka_vm::Vm, func: &FuncTemplate) -> f64 {
    let ret = vm.run_func(func).expect("解释器执行");
    to_f64(ret)
}

/// jitdiff 主体：3200 例生成式差分，解释器 vs JIT 逐位相等。
#[test]
fn jitdiff_3200_generated_cases_zero_mismatch() {
    let mut rng = Rng::new(0x4D35_2026);
    let mut mismatch = 0usize;
    let mut executed = 0usize;
    // 解释用共享 VM：`Vm::new` 会在堆上预建内置原型与构造器单例，
    // 每例重建一次是纯浪费；单例语义对所有用例通用，故循环外建一次复用。
    let mut interp_vm = aluka_vm::Vm::new(0);
    // 编译用共享 VM（模块 Import 解析 helper 地址；运行走零 ctx 纯数值路径）
    let mut compile_vm = aluka_vm::Vm::new(0);
    for id in 0..3200usize {
        let func = generate(&mut rng, id);
        let module = BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![func.clone()],
            classes: Vec::new(),
        };
        if let Err(e) = module.verify() {
            eprintln!("--- case {id} code ---");
            for (i, ins) in func.code.iter().enumerate() {
                eprintln!("{i:3}: {:?} op {}", ins.op, ins.operand);
            }
            panic!("case {id} 未过静态校验: {e:?}");
        }
        let expected = interp_run(&mut interp_vm, &func);
        let consts = std::rc::Rc::new(func.constants.clone());
        let compile_ctx = compile_vm.build_jit_ctx(&consts);
        let jit = match jit_compile(&func, &compile_ctx.vtable) {
            Ok(j) => j,
            Err(e) => panic!("case {id} JIT 编译失败: {e:?}"),
        };
        let mut run_ctx = compile_vm.build_jit_ctx(&consts);
        let jit_box = jit.call_ctx(&mut run_ctx, &[]);
        let got = box_to_f64(jit_box);
        executed += 1;
        if expected.to_bits() != got.to_bits() {
            mismatch += 1;
            eprintln!("失配 case {id}: interp={expected:?} jit={got:?}");
            if mismatch > 5 {
                break;
            }
        }
    }
    assert_eq!(mismatch, 0, "{executed} 例中存在失配");
    assert!(executed >= 3000, "差分例数须 ≥3000（实际 {executed}）");
}
