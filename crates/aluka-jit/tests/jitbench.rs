//! M5 性能基线：热循环 JIT vs 解释器（方法学：交替执行 + 冷却 + min-of-5）。
//!
//! 负载：countdown 累加循环（`n` 轮 f64 算术），子集内可 JIT。与 node 的
//! 对拍数据由 `.work/evidence/20260905/m5-report.md` 记录（node 侧同语义
//! JS 脚本，跨进程测量）。
//!
//! 本测试断言 JIT **不慢于**解释器（保守门禁，避免机器噪声导致 flaky）；
//! 具体倍数写进证据报告。

use aluka_bytecode::{Constant, FuncTemplate, Instr, Op};
use aluka_jit::jit_compile;
use std::time::Instant;

/// 构造热循环：`n = N; acc = 0; while n > 0 { acc = acc + n * 1.5; n -= 1 } return acc`。
fn hot_loop(iterations: f64) -> FuncTemplate {
    let consts = vec![
        Constant::Number(0.0),        // 0
        Constant::Number(1.0),        // 1
        Constant::Number(iterations), // 2
        Constant::Number(1.5),        // 3
    ];
    let mut code = vec![
        Instr::new(Op::PushConst, 2), // n = N
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::PushConst, 0), // acc = 0
        Instr::new(Op::StoreLocal, 2),
    ];
    let loop_start = code.len();
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::PushConst, 0));
    code.push(Instr::new(Op::Gt, 0));
    let jfp_at = code.len();
    code.push(Instr::new(Op::JmpFalsePop, 0));
    // acc = acc + n * 1.5
    code.push(Instr::new(Op::LoadLocal, 2));
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::PushConst, 3));
    code.push(Instr::new(Op::Mul, 0));
    code.push(Instr::new(Op::Add, 0));
    code.push(Instr::new(Op::StoreLocal, 2));
    // n -= 1
    code.push(Instr::new(Op::LoadLocal, 1));
    code.push(Instr::new(Op::PushConst, 1));
    code.push(Instr::new(Op::Sub, 0));
    code.push(Instr::new(Op::StoreLocal, 1));
    let jmp_at = code.len();
    code.push(Instr::new(Op::Jmp, 0));
    let exit_pc = code.len();
    code.push(Instr::new(Op::LoadLocal, 2));
    code.push(Instr::new(Op::Return, 0));

    let patch = |code: &mut Vec<Instr>, at: usize, target: usize| {
        let signed = (target as i32 * 4) - ((at as i32 * 4) + 4);
        code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
    };
    patch(&mut code, jfp_at, exit_pc);
    patch(&mut code, jmp_at, loop_start);

    FuncTemplate {
        name: "hot_loop".to_owned(),
        num_params: 0,
        num_locals: 3,
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

/// min-of-N 计时（毫秒）。
fn min_of<F: FnMut() -> f64>(rounds: usize, mut f: F) -> (f64, f64) {
    let mut best = f64::INFINITY;
    let mut last = 0.0;
    for _ in 0..rounds {
        let t0 = Instant::now();
        last = f();
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        if dt < best {
            best = dt;
        }
        std::thread::sleep(std::time::Duration::from_millis(50)); // 冷却
    }
    (best, last)
}

/// 热循环基线：JIT 与解释器结果一致，且 JIT 不慢于解释器。
#[test]
fn hot_loop_jit_not_slower_than_interpreter() {
    let iterations = 200_000.0;
    let func = hot_loop(iterations);
    // 编译需 vtable（模块 Import 解析 helper 地址）；运行走零 ctx 纯数值路径
    let mut compile_vm = aluka_vm::Vm::new(0);
    let consts = std::rc::Rc::new(func.constants.clone());
    let compile_ctx = compile_vm.build_jit_ctx(&consts);
    let jit = jit_compile(&func, &compile_ctx.vtable).expect("JIT 编译");

    // 交替执行 + 冷却 + min-of-5（总 TODO §1 性能方法学）
    let mut interp_best = f64::INFINITY;
    let mut jit_best = f64::INFINITY;
    let mut interp_val = 0.0;
    let mut jit_val = 0.0;
    for _ in 0..5 {
        let (i_ms, i_v) = min_of(1, || {
            let mut vm = aluka_vm::Vm::new(0);
            vm.run_func(&func)
                .expect("解释执行")
                .as_number()
                .unwrap_or(f64::NAN)
        });
        if i_ms < interp_best {
            interp_best = i_ms;
        }
        interp_val = i_v;
        let (j_ms, j_v) = min_of(1, || jit.call(&[]));
        if j_ms < jit_best {
            jit_best = j_ms;
        }
        jit_val = j_v;
    }

    assert_eq!(
        interp_val.to_bits(),
        jit_val.to_bits(),
        "结果必须逐位一致（interp={interp_val} jit={jit_val}）"
    );
    println!(
        "hot_loop({iterations} 轮) min-of-5: interp={interp_best:.2}ms jit={jit_best:.2}ms 加速={:.1}×",
        interp_best / jit_best.max(f64::MIN_POSITIVE)
    );
    assert!(
        jit_best <= interp_best,
        "JIT 不应慢于解释器（interp={interp_best:.2}ms jit={jit_best:.2}ms）"
    );
}

/// 构建属性求和热循环字节码：`o={x:1,y:2}; s=0; i=N; while i>0 { s+=o.x+o.y; i-=1 }`
/// 返回 s。属性 x/y 恒定 → 解释器侧 200k 轮 propAccess 基线。
fn prop_sum(iterations: f64) -> FuncTemplate {
    let x_name = 0u32;
    let y_name = 1u32;
    // 0="x" 1="y" 2=x值(1) 3=y值(2) 4=零 5=步长(1) 6=迭代数
    let consts: Vec<Constant> = vec![
        Constant::String("x".to_owned()),
        Constant::String("y".to_owned()),
        Constant::Number(1.0),
        Constant::Number(2.0),
        Constant::Number(0.0),
        Constant::Number(1.0),
        Constant::Number(iterations),
    ];
    let mut code = vec![
        Instr::new(Op::NewObject, 0),
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 2),
        Instr::new(Op::SetProp, x_name),
        Instr::new(Op::Pop, 0),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 3),
        Instr::new(Op::SetProp, y_name),
        Instr::new(Op::Pop, 0),
        Instr::new(Op::PushConst, 4),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::PushConst, 6),
        Instr::new(Op::StoreLocal, 3),
    ];
    let head = code.len();
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 3),
        Instr::new(Op::PushConst, 4),
        Instr::new(Op::Gt, 0),
        Instr::new(Op::JmpFalsePop, 0),
    ]);
    let false_pc = code.len() - 1;
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::GetProp, x_name),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::GetProp, y_name),
        Instr::new(Op::Add, 0),
        Instr::new(Op::Add, 0),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::LoadLocal, 3),
        Instr::new(Op::PushConst, 5),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::StoreLocal, 3),
        Instr::new(Op::Jmp, 0),
    ]);
    let jmp_pc = code.len() - 1;
    code.push(Instr::new(Op::LoadLocal, 2));
    let exit = code.len() - 1;
    code.push(Instr::new(Op::Return, 0));
    let patch = |code: &mut Vec<Instr>, at: usize, target: usize| {
        let signed = (target as i32 * 4) - ((at as i32 * 4) + 4);
        code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
    };
    patch(&mut code, false_pc, exit);
    patch(&mut code, jmp_pc, head);
    FuncTemplate {
        name: "prop_sum".to_owned(),
        num_params: 0,
        num_locals: 4,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack: 64,
        source_file: String::new(),
        constants: consts,
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    }
}

/// propAccess 对拍：JIT-PIC vs 解释器（in-process，交替 + 冷却 50ms + min-of-5）。
/// node 22 对照见 evidence（跨进程另测）。
#[test]
fn prop_sum_pic_vs_interpreter() {
    let iterations = 200_000.0;
    let func = prop_sum(iterations);
    let mut compile_vm = aluka_vm::Vm::new(0);
    let consts = std::rc::Rc::new(func.constants.clone());
    let compile_ctx = compile_vm.build_jit_ctx(&consts);
    let jit = jit_compile(&func, &compile_ctx.vtable).expect("JIT 编译 prop_sum");

    let mut interp_best = f64::INFINITY;
    let mut jit_best = f64::INFINITY;
    let mut interp_val = 0.0;
    let mut jit_val = 0.0;
    for _ in 0..5 {
        let (i_ms, i_v) = min_of(1, || {
            let mut vm = aluka_vm::Vm::new(0);
            vm.set_jit_enabled(false);
            vm.run_func(&func)
                .expect("解释执行 prop_sum")
                .as_number()
                .unwrap_or(f64::NAN)
        });
        if i_ms < interp_best {
            interp_best = i_ms;
        }
        interp_val = i_v;
        let (j_ms, j_v) = min_of(1, || {
            let mut vm = aluka_vm::Vm::new(0);
            let consts = std::rc::Rc::new(func.constants.clone());
            let mut ctx = vm.build_jit_ctx(&consts);
            let b = jit.call_ctx(&mut ctx, &[]);
            if aluka_jit::valbox::is_number(b) {
                aluka_jit::valbox::unbox_number(b)
            } else {
                f64::NAN
            }
        });
        if j_ms < jit_best {
            jit_best = j_ms;
        }
        jit_val = j_v;
    }

    assert_eq!(interp_val, 600000.0, "解释器 propSum 应为 600000");
    assert_eq!(
        interp_val.to_bits(),
        jit_val.to_bits(),
        "JIT 与解释器结果须逐位一致（interp={interp_val} jit={jit_val}）"
    );
    println!(
        "prop_sum(200k o.x+o.y) min-of-5: interp={interp_best:.2}ms jit={jit_best:.2}ms 加速={:.1}×（node 22 预热 TurboFan ≈0.09–0.13ms，见 evidence/node-jitbench.js）",
        interp_best / jit_best.max(f64::MIN_POSITIVE)
    );
    assert!(
        jit_best <= interp_best,
        "JIT-PIC 不应慢于解释器（interp={interp_best:.2}ms jit={jit_best:.2}ms）"
    );
}

/// 构建 closureCall 热循环：`s=0; i=N; while i>0 { s += callee(i); i-=1 }`，
/// 被调 `callee(n) { return n * 1 }`（经全局取用；JIT 侧 CALL 回调解释器）。
fn closure_call_loop(iterations: f64) -> (FuncTemplate, FuncTemplate) {
    // 常量：0="callee" 1=零 2=步长1 3=迭代数
    let consts: Vec<Constant> = vec![
        Constant::String("callee".to_owned()),
        Constant::Number(0.0),
        Constant::Number(1.0),
        Constant::Number(iterations),
    ];
    let mut code = vec![
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::StoreLocal, 1), // s = 0
        Instr::new(Op::PushConst, 3),
        Instr::new(Op::StoreLocal, 2), // i = N
    ];
    let head = code.len();
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::Gt, 0),
        Instr::new(Op::JmpFalsePop, 0),
    ]);
    let false_pc = code.len() - 1;
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::LoadGlobal, 0),
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::Call, 1),
        Instr::new(Op::Add, 0),
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::PushConst, 2),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::Jmp, 0),
    ]);
    let jmp_pc = code.len() - 1;
    code.push(Instr::new(Op::LoadLocal, 1));
    let exit = code.len() - 1;
    code.push(Instr::new(Op::Return, 0));
    let patch = |code: &mut Vec<Instr>, at: usize, target: usize| {
        let signed = (target as i32 * 4) - ((at as i32 * 4) + 4);
        code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
    };
    patch(&mut code, false_pc, exit);
    patch(&mut code, jmp_pc, head);
    let caller = FuncTemplate {
        name: "closure_call_loop".to_owned(),
        num_params: 0,
        num_locals: 3,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack: 64,
        source_file: String::new(),
        constants: consts,
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    };
    let callee = FuncTemplate {
        name: "callee".to_owned(),
        num_params: 1,
        num_locals: 2,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code: vec![
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::Mul, 0),
            Instr::new(Op::Return, 0),
        ],
        max_stack: 8,
        source_file: String::new(),
        constants: vec![Constant::Number(1.0)],
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    };
    (caller, callee)
}

/// closureCall 对拍：JIT（`CALL` 首轮登记调用 IC，其后原生直调被调机器码）
/// vs 纯解释器（in-process，交替 + 冷却 50ms + min-of-5）。node 对照见 evidence。
#[test]
fn closure_call_jit_vs_interpreter() {
    let iterations = 200_000.0;
    let (caller, callee) = closure_call_loop(iterations);
    let module = aluka_bytecode::BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), callee],
        classes: Vec::new(),
    };
    // 期望值：sum(1..=N) = N*(N+1)/2
    let expected = iterations * (iterations + 1.0) / 2.0;

    let setup = |vm: &mut aluka_vm::Vm| {
        vm.load_module_for_test(&module);
        let closure = vm.alloc_closure(1);
        vm.globals
            .insert("callee".to_owned(), aluka_vm::Value::Object(closure));
    };
    // 被调跨热点阈值 → 编译为机器码，使调用 IC 可登记原生入口
    let warm = |vm: &mut aluka_vm::Vm| {
        for _ in 0..(aluka_vm::JIT_HOT_THRESHOLD + 1) {
            let _ = vm.invoke_function(
                1,
                aluka_vm::Value::Undefined,
                &[aluka_vm::Value::Number(1.0)],
                Vec::new(),
            );
        }
    };

    let mut compile_vm = aluka_vm::Vm::new(0);
    setup(&mut compile_vm);
    let consts = std::rc::Rc::new(caller.constants.clone());
    let compile_ctx = compile_vm.build_jit_ctx(&consts);
    let jit = jit_compile(&caller, &compile_ctx.vtable).expect("JIT 编译 closure_call_loop");

    let mut interp_best = f64::INFINITY;
    let mut jit_best = f64::INFINITY;
    let mut interp_val = 0.0;
    let mut jit_val = 0.0;
    let mut fallbacks = u64::MAX;
    for _ in 0..5 {
        let (i_ms, i_v) = min_of(1, || {
            let mut vm = aluka_vm::Vm::new(0);
            vm.set_jit_enabled(false);
            setup(&mut vm);
            vm.run_func(&caller)
                .expect("解释执行 closure_call_loop")
                .as_number()
                .unwrap_or(f64::NAN)
        });
        if i_ms < interp_best {
            interp_best = i_ms;
        }
        interp_val = i_v;
        let mut vm = aluka_vm::Vm::new(0);
        setup(&mut vm);
        warm(&mut vm);
        let consts = std::rc::Rc::new(caller.constants.clone());
        let mut ctx = vm.build_jit_ctx(&consts);
        let before = vm.jit_call_fallbacks();
        let (j_ms, j_v) = min_of(1, || {
            let b = jit.call_ctx(&mut ctx, &[]);
            if aluka_jit::valbox::is_number(b) {
                aluka_jit::valbox::unbox_number(b)
            } else {
                f64::NAN
            }
        });
        fallbacks = vm.jit_call_fallbacks() - before;
        if j_ms < jit_best {
            jit_best = j_ms;
        }
        jit_val = j_v;
    }

    assert_eq!(
        interp_val, expected,
        "解释器 closureCall 累加应为 {expected}"
    );
    assert_eq!(
        interp_val.to_bits(),
        jit_val.to_bits(),
        "JIT 与解释器结果须逐位一致（interp={interp_val} jit={jit_val}）"
    );
    // 直调覆盖率是可观测量，不靠计时推断：200k 轮只应首轮进 helper
    assert_eq!(
        fallbacks, 1,
        "调用 IC 命中后应原生直调，仅首轮落 helper（实际 {fallbacks}）"
    );
    println!(
        "closure_call({iterations} 轮 callee(i)) min-of-5: interp={interp_best:.2}ms jit={jit_best:.2}ms 加速={:.1}×（helper 回退 {fallbacks} 次）",
        interp_best / jit_best.max(f64::MIN_POSITIVE)
    );
}
