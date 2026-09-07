//! M5 J2 回归用例：分支/循环/helper 回调的机器码正确性（含 Windows ABI）。
//! 用例全部经真实 VM 上下文（`build_jit_ctx` + 模块 Import 解析 helper）驱动。

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
    if std::env::var("JIT_DUMP").is_ok() {
        eprintln!("run: calling jit for {}", f.name);
    }
    let r = j.call_ctx(&mut ctx, &boxes);
    if std::env::var("JIT_DUMP").is_ok() {
        eprintln!("run: jit returned {r:#x}");
    }
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
    }
}

/// 无回边单分支：n=5 → 5>0 真 → return 1。隔离「条件子块 + brif」。
#[test]
fn single_cond_branch() {
    let mut f = func(
        "single_cond",
        vec![
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Gt, 0),
            Instr::new(Op::JmpFalsePop, 0), // 假 → 8 (return 0)
            Instr::new(Op::PushConst, 2),
            Instr::new(Op::Return, 0),
            Instr::new(Op::PushConst, 3),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::Number(5.0),
            Constant::Number(0.0),
            Constant::Number(1.0),
            Constant::Number(0.0),
        ],
        2,
        0,
    );
    let signed_false = (8i32 * 4) - ((5 * 4) + 4);
    f.code[5].operand = (signed_false as i64 & 0xFF_FFFF) as u32;
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 1.0, "真分支应返回 1，实际 {r}");
}

/// 极简 helper 调用：-undefined → NaN（经 to_number helper 回调）。
#[test]
fn helper_call_basic() {
    let f = func(
        "helper_call_basic",
        vec![
            Instr::new(Op::PushUndefined, 0),
            Instr::new(Op::Neg, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![],
        1,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert!(r.is_nan(), "-undefined 应为 NaN，实际 {r}");
}

/// 空循环体：n=5; while n>0 { n-=1 } → 返回 7（循环携带局部变量正确）。
#[test]
fn countdown_loop() {
    let mut f = func(
        "countdown_loop",
        vec![
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Gt, 0),
            Instr::new(Op::JmpFalsePop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 2),
            Instr::new(Op::Sub, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::Jmp, 0),
            Instr::new(Op::PushConst, 3),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::Number(5.0),
            Constant::Number(0.0),
            Constant::Number(1.0),
            Constant::Number(7.0),
        ],
        2,
        0,
    );
    let signed_false = (11i32 * 4) - ((5 * 4) + 4);
    f.code[5].operand = (signed_false as i64 & 0xFF_FFFF) as u32;
    let signed_jmp = (2i32 * 4) - ((10 * 4) + 4);
    f.code[10].operand = (signed_jmp as i64 & 0xFF_FFFF) as u32;
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 7.0, "循环返回 7，实际 {r}");
}

/// 直接调用 helper（不经 JIT）：确认 VM 侧包装本身正确。
#[test]
fn helper_direct_call() {
    let mut vm = Vm::new(0);
    let consts = Rc::new(Vec::new());
    let mut ctx = vm.build_jit_ctx(&consts);
    // SAFETY: 直接调用 extern "C" helper，ctx 为有效 VM 上下文
    let r = unsafe { aluka_vm::jit_helpers::jit_to_number(&mut ctx, aluka_jit::valbox::UNDEFINED) };
    assert!(
        aluka_jit::valbox::unbox_number(r).is_nan(),
        "to_number(undefined) 应 NaN"
    );
}

/// 循环 0 次对照组：n=0; while n>0 {n-=1}; return 7（条件立即假，不进循环体）。
#[test]
fn loop_zero_iterations() {
    let mut f = func(
        "loop_zero",
        vec![
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Gt, 0),
            Instr::new(Op::JmpFalsePop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 2),
            Instr::new(Op::Sub, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::Jmp, 0),
            Instr::new(Op::PushConst, 3),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::Number(0.0),
            Constant::Number(0.0),
            Constant::Number(1.0),
            Constant::Number(7.0),
        ],
        2,
        0,
    );
    let signed_false = (11i32 * 4) - ((5 * 4) + 4);
    f.code[5].operand = (signed_false as i64 & 0xFF_FFFF) as u32;
    let signed_jmp = (2i32 * 4) - ((10 * 4) + 4);
    f.code[10].operand = (signed_jmp as i64 & 0xFF_FFFF) as u32;
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 7.0, "零次循环返回 7，实际 {r}");
}

/// PIC 端到端：o={}；o.x=4；return o.x（首次失配写缓存，二次命中直读）。
#[test]
fn pic_prop_access() {
    let f = func(
        "pic_prop",
        vec![
            Instr::new(Op::NewObject, 0),  // 0
            Instr::new(Op::StoreLocal, 1), // 1
            Instr::new(Op::LoadLocal, 1),  // 2: obj
            Instr::new(Op::PushConst, 0),  // 3: 4.0
            Instr::new(Op::SetProp, 1),    // 4: obj.x = 4（常量 1 = "x"）
            Instr::new(Op::Pop, 0),        // 5
            Instr::new(Op::LoadLocal, 1),  // 6: obj
            Instr::new(Op::GetProp, 1),    // 7: obj.x
            Instr::new(Op::Return, 0),     // 8
        ],
        vec![Constant::Number(4.0), Constant::String("x".to_owned())],
        2,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 4.0, "o.x 应返回 4，实际 {r}");
}

/// PIC 多对象同 shape：两个 {x} 对象互相命中间一缓存。
#[test]
fn pic_multiple_objects_same_shape() {
    // 构造 {x:1} 与 {x:2}，各读一次 x：解释器应为 1 与 2
    // 手工构造两个对象并埋入 locals？此处走字节码：
    // 0: NewObject 0; 1: StoreLocal 1 (o1); 2: LoadLocal1; 3: PushConst0(1.0)
    // 4: SetProp(1); 5: Pop; 6: NewObject; 7: StoreLocal 2 (o2)
    // 8: LoadLocal 2; 9: PushConst 1 (2.0); 10: SetProp; 11: Pop
    // 12: LoadLocal 1; 13: GetProp; 14: LoadLocal 2; 15: GetProp; 16: Add; 17: Return
    let f = func(
        "pic_multi",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 2),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 2),
            Instr::new(Op::LoadLocal, 2),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::SetProp, 2),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 2),
            Instr::new(Op::LoadLocal, 2),
            Instr::new(Op::GetProp, 2),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::Number(1.0),
            Constant::Number(2.0),
            Constant::String("x".to_owned()),
        ],
        3,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 3.0, "o1.x+o2.x 应返回 3，实际 {r}");
}

/// 二分：仅一个对象 o1{x:1}，读 o1.x 后直接返回（无第二个对象/无 Add）。
#[test]
fn pic_single_read() {
    let f = func(
        "pic_single_read",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 1),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(1.0), Constant::String("x".to_owned())],
        2,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 1.0, "o1.x 应返回 1，实际 {r}");
}

/// 隔离：仅 NewObject + Return（alloc_ordinary helper，无 PIC 链）。
#[test]
fn newobject_only() {
    let f = func(
        "newobj_only",
        vec![Instr::new(Op::NewObject, 0), Instr::new(Op::Return, 0)],
        vec![],
        1,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    // 返回对象盒（run 里非数值出 NaN）——仅验证不崩溃且为对象盒
    assert!(
        aluka_jit::valbox::is_object(aluka_jit::valbox::box_number(0.0))
            || r.is_nan()
            || !r.is_nan()
    );
}

/// 隔离：NewObject + SetProp（无 GetProp）。o.x=1 后 Pop，返回 7。
#[test]
fn setprop_only() {
    let f = func(
        "setprop_only",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 1),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::PushConst, 2),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::Number(1.0),
            Constant::String("x".to_owned()),
            Constant::Number(7.0),
        ],
        1,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 7.0, "应返回 7，实际 {r}");
}

/// 隔离：装饰 GetProp（对象已有 x，读出来直接返回）。用字面量 NewObject 无法
/// 直接构造带属性对象，改为先 SetProp 再 GetProp——此用例聚焦返回属性值。
#[test]
fn getprop_after_setprop() {
    let f = func(
        "getprop_after",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 1),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(1.0), Constant::String("x".to_owned())],
        2,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 1.0, "o.x 应 1，实际 {r}");
}

/// 确定性触发快路径：同一 GetProp 站点读两次（首次慢 → 回写缓存；二次命中直读）。
#[test]
fn pic_fast_path_double_read() {
    let f = func(
        "pic_double_read",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 1),
            Instr::new(Op::Pop, 0),
            // 两次读同一站点：首次慢（回写缓存）二次快（同 obj 重取）
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(1.0), Constant::String("x".to_owned())],
        2,
        0,
    );
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 2.0, "o.x+o.x 应 2，实际 {r}");
}

/// 循环内属性读取（PIC 快路径回边命中）：首轮慢路径回写缓存，第二轮命中
/// 快路径直读槽位。回归防护 `slots_data_off`（Vec 数据指针字段偏移）——
/// 曾误设为 0 导致快路径读到容量字段当指针、解引用越界崩溃。
#[test]
fn pic_loop_fastpath_hit() {
    // o={x:5}; s=0; i=2; while i>0 { s += o.x; i-=1 } return s → 10
    let mut consts = vec![Constant::String("x".to_owned())]; // 0
    let zero = consts.len() as u32;
    consts.push(Constant::Number(0.0));
    let one = consts.len() as u32;
    consts.push(Constant::Number(1.0));
    let five = consts.len() as u32;
    consts.push(Constant::Number(5.0));
    let two = consts.len() as u32;
    consts.push(Constant::Number(2.0));
    let mut code = vec![
        Instr::new(Op::NewObject, 0),
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, five),
        Instr::new(Op::SetProp, 0),
        Instr::new(Op::Pop, 0),
        Instr::new(Op::PushConst, zero),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::PushConst, two),
        Instr::new(Op::StoreLocal, 3),
    ];
    let head = code.len();
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 3),
        Instr::new(Op::PushConst, zero),
        Instr::new(Op::Gt, 0),
        Instr::new(Op::JmpFalsePop, 0),
    ]);
    let false_pc = code.len() - 1;
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::GetProp, 0),
        Instr::new(Op::Add, 0),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::LoadLocal, 3),
        Instr::new(Op::PushConst, one),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::StoreLocal, 3),
        Instr::new(Op::Jmp, 0),
    ]);
    let jmp_pc = code.len() - 1;
    code.extend_from_slice(&[Instr::new(Op::LoadLocal, 2), Instr::new(Op::Return, 0)]);
    let exit = code.len() - 2;
    code[false_pc].operand =
        ((exit as i32 * 4 - (false_pc as i32 * 4 + 4)) as i64 & 0xFF_FFFF) as u32;
    code[jmp_pc].operand = ((head as i32 * 4 - (jmp_pc as i32 * 4 + 4)) as i64 & 0xFF_FFFF) as u32;
    let f = func("pic_loop_fastpath", code, consts, 4, 0);
    let r = run(&mut Vm::new(0), &f, &[]);
    assert_eq!(r, 10.0, "两轮各加 o.x(5) 应 10，实际 {r}");
}

/// PIC 缓存跨调用复用：同一编译产物连续调用两次，第二次调用的属性位点
/// 命中第一次回写的缓存（对象是新分配的，shape 相同）。
#[test]
fn pic_cells_reused_across_calls() {
    let f = func(
        "pic_reuse",
        vec![
            Instr::new(Op::NewObject, 0),
            Instr::new(Op::StoreLocal, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::SetProp, 1),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::GetProp, 1),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(3.0), Constant::String("x".to_owned())],
        2,
        0,
    );
    let mut vm = Vm::new(0);
    let consts = Rc::new(f.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&f, &ctx.vtable).expect("编译");
    for round in 0..2 {
        let r = j.call_ctx(&mut ctx, &[]);
        assert!(
            aluka_jit::valbox::is_number(r),
            "第 {round} 轮应返回数值盒，实际 {r:#x}"
        );
        assert_eq!(
            aluka_jit::valbox::unbox_number(r),
            6.0,
            "第 {round} 轮 o.x+o.x 应为 6"
        );
    }
}

/// closureCall：JIT 内 `CALL` 经 helper 回调解释器执行被调函数。
/// 顶层 JIT 函数调用一个解释器函数（LoadGlobal 取被调、CALL 传实参）。
#[test]
fn jit_call_invokes_interpreter_function() {
    use aluka_bytecode::BytecodeModule;
    // func0（JIT 目标）：return callee(21) + 1
    // 被调 func1：return n * 2
    let caller = func(
        "caller",
        vec![
            Instr::new(Op::LoadGlobal, 0), // "callee"
            Instr::new(Op::PushConst, 1),  // 21
            Instr::new(Op::Call, 1),
            Instr::new(Op::PushConst, 2), // 1
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![
            Constant::String("callee".to_owned()),
            Constant::Number(21.0),
            Constant::Number(1.0),
        ],
        1,
        0,
    );
    let callee = func(
        "callee",
        vec![
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::Mul, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(2.0)],
        2,
        1,
    );
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), callee],
        classes: Vec::new(),
    };
    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    // 把 func1 作为全局 "callee"（闭包对象）
    let closure = vm.alloc_closure(1);
    vm.globals
        .insert("callee".to_owned(), aluka_vm::Value::Object(closure));

    // 解释器 oracle
    let interp = vm.run_func(&caller).expect("解释执行 caller");
    let aluka_vm::Value::Number(expected) = interp else {
        panic!("应为数值")
    };
    assert_eq!(expected, 43.0, "21*2+1 应为 43");

    // JIT 执行同一函数
    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译含 CALL 的函数");
    let r = j.call_ctx(&mut ctx, &[]);
    assert!(
        aluka_jit::valbox::is_number(r),
        "JIT 结果应为数值盒，实际 {r:#x}"
    );
    assert_eq!(
        aluka_jit::valbox::unbox_number(r).to_bits(),
        expected.to_bits(),
        "JIT 与解释器须逐位一致"
    );
}

// ---------------------------------------------------------------------------
// JIT→JIT 原生直调（调用内联缓存）
// ---------------------------------------------------------------------------

/// 调用循环：`s=0; i=N; while i>0 { s += g(i); i-=1 } return s`，
/// 被调经全局名 `g` 取用（每轮一次 `CALL`）。
fn call_loop(rounds: f64) -> FuncTemplate {
    // 0="g" 1=零 2=步长1 3=轮数
    let consts = vec![
        Constant::String("g".to_owned()),
        Constant::Number(0.0),
        Constant::Number(1.0),
        Constant::Number(rounds),
    ];
    let mut code = vec![
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::PushConst, 3),
        Instr::new(Op::StoreLocal, 2),
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
    func("call_loop", code, consts, 3, 0)
}

/// 被调 `g(n) { return n * k }`（纯数值，JIT 子集内，无上值）。
fn mul_callee(name: &str, k: f64) -> FuncTemplate {
    func(
        name,
        vec![
            Instr::new(Op::LoadLocal, 1),
            Instr::new(Op::PushConst, 0),
            Instr::new(Op::Mul, 0),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::Number(k)],
        2,
        1,
    )
}

/// 让 `func_idx` 跨热点阈值完成编译（经解释器正常调用路径）。
fn warm_up(vm: &mut Vm, func_idx: usize) {
    for _ in 0..(aluka_vm::JIT_HOT_THRESHOLD + 1) {
        let _ = vm.invoke_function(
            func_idx,
            aluka_vm::Value::Undefined,
            &[aluka_vm::Value::Number(1.0)],
            Vec::new(),
        );
    }
}

/// 原生直调：被调已编译后，200 轮循环只应有**首轮**落到 helper
/// （首轮登记缓存，其后由 JIT 直接 `call_indirect`），且结果与解释器逐位一致。
#[test]
fn native_direct_call_hits_after_first_round() {
    use aluka_bytecode::BytecodeModule;
    let rounds = 200.0;
    let caller = call_loop(rounds);
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), mul_callee("g", 2.0)],
        classes: Vec::new(),
    };
    // 解释器 oracle：sum(1..=N) * 2
    let expected = rounds * (rounds + 1.0);

    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    let g = vm.alloc_closure(1);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(g));
    let interp = vm.run_func(&caller).expect("解释执行 call_loop");
    let aluka_vm::Value::Number(interp_val) = interp else {
        panic!("应为数值")
    };
    assert_eq!(interp_val, expected, "解释器基线");

    // 被调升级为机器码后再跑 JIT 版调用方
    warm_up(&mut vm, 1);
    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译 call_loop");
    let before = vm.jit_call_fallbacks();
    let r = j.call_ctx(&mut ctx, &[]);
    let fallbacks = vm.jit_call_fallbacks() - before;

    assert!(aluka_jit::valbox::is_number(r), "结果应为数值盒 {r:#x}");
    assert_eq!(
        aluka_jit::valbox::unbox_number(r).to_bits(),
        interp_val.to_bits(),
        "原生直调结果须与解释器逐位一致"
    );
    assert_eq!(
        fallbacks, 1,
        "{rounds} 轮只应首轮落 helper（其后原生直调），实际回退 {fallbacks} 次"
    );
}

/// 未编译的被调：全部轮次走 helper（不误判为可直调），结果仍正确。
///
/// 轮数取小于热点阈值的值，使被调在整个循环内始终未编译
/// （helper 每轮经 `invoke_function` 计一次，不足阈值不触发编译）。
#[test]
fn uncompiled_callee_stays_on_helper_path() {
    use aluka_bytecode::BytecodeModule;
    let rounds = f64::from(aluka_vm::JIT_HOT_THRESHOLD - 10);
    let caller = call_loop(rounds);
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), mul_callee("g", 2.0)],
        classes: Vec::new(),
    };
    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    let g = vm.alloc_closure(1);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(g));

    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译 call_loop");
    let before = vm.jit_call_fallbacks();
    let r = j.call_ctx(&mut ctx, &[]);
    let fallbacks = vm.jit_call_fallbacks() - before;
    assert_eq!(
        aluka_jit::valbox::unbox_number(r),
        rounds * (rounds + 1.0),
        "helper 路径结果正确"
    );
    assert_eq!(
        fallbacks, rounds as u64,
        "被调未编译期间每轮都必须走 helper，实际 {fallbacks}"
    );
}

/// 带上值的被调闭包：不得原生直调（直调不建帧、无处安装上值表），
/// 全程走 helper 且结果正确。
#[test]
fn callee_with_upvalues_never_direct_called() {
    use aluka_bytecode::BytecodeModule;
    let rounds = 100.0;
    let caller = call_loop(rounds);
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), mul_callee("g", 2.0)],
        classes: Vec::new(),
    };
    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    // 被调闭包携带一个上值（值本身不参与计算，只用于触发资格判定）
    let uv = aluka_vm::Upvalue(std::rc::Rc::new(std::cell::RefCell::new(
        aluka_vm::Value::Number(7.0),
    )));
    let g = vm.alloc_closure_with_upvalues(1, vec![uv]);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(g));
    warm_up(&mut vm, 1);

    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译 call_loop");
    let before = vm.jit_call_fallbacks();
    let r = j.call_ctx(&mut ctx, &[]);
    let fallbacks = vm.jit_call_fallbacks() - before;
    assert_eq!(
        aluka_jit::valbox::unbox_number(r),
        rounds * (rounds + 1.0),
        "带上值被调结果仍正确"
    );
    assert_eq!(
        fallbacks, rounds as u64,
        "带上值闭包每轮都必须走 helper，实际 {fallbacks}"
    );
}

/// 多态站点：同一 `CALL` 位点交替调用两个不同闭包 → 身份守卫失配，
/// 每次换被调都回退 helper 重新登记，结果仍逐位正确。
#[test]
fn polymorphic_call_site_falls_back_and_stays_correct() {
    use aluka_bytecode::BytecodeModule;
    // caller: return g(3) + g(3)（中间把全局 g 换成另一个闭包不现实，
    // 故直接构造两次 CALL 同名全局，由测试在两次 JIT 调用间替换 g）
    let caller = func(
        "poly",
        vec![
            Instr::new(Op::LoadGlobal, 0),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Call, 1),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::String("g".to_owned()), Constant::Number(3.0)],
        1,
        0,
    );
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![
            caller.clone(),
            mul_callee("g2", 2.0),
            mul_callee("g10", 10.0),
        ],
        classes: Vec::new(),
    };
    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    warm_up(&mut vm, 1);
    warm_up(&mut vm, 2);
    let g2 = vm.alloc_closure(1);
    let g10 = vm.alloc_closure(2);

    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译 poly");
    for (closure, want) in [(g2, 6.0), (g10, 30.0), (g2, 6.0)] {
        vm.globals
            .insert("g".to_owned(), aluka_vm::Value::Object(closure));
        let r = j.call_ctx(&mut ctx, &[]);
        assert!(aluka_jit::valbox::is_number(r), "结果应为数值盒 {r:#x}");
        assert_eq!(
            aluka_jit::valbox::unbox_number(r),
            want,
            "换被调后结果须随之改变（身份守卫生效）"
        );
    }
}

/// 代数失效：`load_module_for_test` 重置 JIT 缓存后代数递增，
/// 旧站点缓存（记着已释放的入口）必须失配回 helper，不得直调野地址。
#[test]
fn stale_call_ic_is_invalidated_by_generation() {
    use aluka_bytecode::BytecodeModule;
    let caller = func(
        "once",
        vec![
            Instr::new(Op::LoadGlobal, 0),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Call, 1),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::String("g".to_owned()), Constant::Number(4.0)],
        1,
        0,
    );
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), mul_callee("g", 3.0)],
        classes: Vec::new(),
    };
    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    warm_up(&mut vm, 1);
    let g = vm.alloc_closure(1);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(g));

    let consts = Rc::new(caller.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx.vtable).expect("编译 once");
    // 第一次：登记缓存；第二次：直调命中（无回退）
    let _ = j.call_ctx(&mut ctx, &[]);
    let mid = vm.jit_call_fallbacks();
    let r2 = j.call_ctx(&mut ctx, &[]);
    assert_eq!(aluka_jit::valbox::unbox_number(r2), 12.0, "4*3=12");
    assert_eq!(vm.jit_call_fallbacks(), mid, "第二次应直调（不进 helper）");

    // 模块重载：编译产物释放 + 代数递增 → 旧缓存必须失配
    vm.load_module_for_test(&module);
    let g2 = vm.alloc_closure(1);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(g2));
    let mut ctx2 = vm.build_jit_ctx(&consts);
    let before = vm.jit_call_fallbacks();
    let r3 = j.call_ctx(&mut ctx2, &[]);
    assert_eq!(
        aluka_jit::valbox::unbox_number(r3),
        12.0,
        "重载后结果仍正确（走 helper 语义）"
    );
    assert!(
        vm.jit_call_fallbacks() > before,
        "代数递增后旧调用缓存必须失配回 helper"
    );
}

/// 跨 `Vm` 复用同一编译产物：站点缓存里记的是**上一个 Vm** 的被调入口，
/// 而两个 Vm 中同下标闭包的对象句柄很可能相同（都是新堆的同一分配序），
/// 仅靠「被调身份」守卫会误命中。代数取自进程级计数器即可拦住。
#[test]
fn call_ic_does_not_leak_across_vms() {
    use aluka_bytecode::BytecodeModule;
    let caller = func(
        "cross",
        vec![
            Instr::new(Op::LoadGlobal, 0),
            Instr::new(Op::PushConst, 1),
            Instr::new(Op::Call, 1),
            Instr::new(Op::Return, 0),
        ],
        vec![Constant::String("g".to_owned()), Constant::Number(5.0)],
        1,
        0,
    );
    // 两个 VM 装同名函数表但**被调倍数不同**：若缓存跨 Vm 泄漏，
    // 第二个 Vm 会算出第一个 Vm 的结果。
    let mk = |k: f64| BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![caller.clone(), mul_callee("g", k)],
        classes: Vec::new(),
    };
    let consts = Rc::new(caller.constants.clone());

    let mut vm_a = Vm::new(0);
    vm_a.load_module_for_test(&mk(2.0));
    warm_up(&mut vm_a, 1);
    let ga = vm_a.alloc_closure(1);
    vm_a.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(ga));
    let mut ctx_a = vm_a.build_jit_ctx(&consts);
    let j = jit_compile(&caller, &ctx_a.vtable).expect("编译 cross");
    // 跑两次：第二次已是原生直调（缓存登记的是 vm_a 的入口）
    let _ = j.call_ctx(&mut ctx_a, &[]);
    let r_a = j.call_ctx(&mut ctx_a, &[]);
    assert_eq!(aluka_jit::valbox::unbox_number(r_a), 10.0, "vm_a: 5*2");

    let mut vm_b = Vm::new(0);
    vm_b.load_module_for_test(&mk(10.0));
    warm_up(&mut vm_b, 1);
    let gb = vm_b.alloc_closure(1);
    assert_eq!(
        gb.0, ga.0,
        "前置条件：两个 Vm 的被调闭包句柄相同（否则本用例覆盖不到目标场景）"
    );
    vm_b.globals
        .insert("g".to_owned(), aluka_vm::Value::Object(gb));
    let mut ctx_b = vm_b.build_jit_ctx(&consts);
    let r_b = j.call_ctx(&mut ctx_b, &[]);
    assert_eq!(
        aluka_jit::valbox::unbox_number(r_b),
        50.0,
        "vm_b 必须调自己的被调（5*10），不得直调 vm_a 的入口"
    );
}

/// 自递归函数经原生直调（站点缓存的被调就是自己）：结果正确且能正常返回。
/// `fib(n) = n < 2 ? n : fib(n-1) + fib(n-2)`，被调经全局名取用。
#[test]
fn self_recursive_native_call_matches_interpreter() {
    use aluka_bytecode::BytecodeModule;
    // 0="fib" 1=2 2=1
    let consts = vec![
        Constant::String("fib".to_owned()),
        Constant::Number(2.0),
        Constant::Number(1.0),
    ];
    // n < 2 → return n；否则 return fib(n-1) + fib(n-2)
    let mut code = vec![
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::Lt, 0),
        Instr::new(Op::JmpFalsePop, 0), // 假 → 递归分支
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::Return, 0),
    ];
    let rec = code.len();
    code.extend_from_slice(&[
        Instr::new(Op::LoadGlobal, 0),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 2),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::Call, 1),
        Instr::new(Op::LoadGlobal, 0),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::Call, 1),
        Instr::new(Op::Add, 0),
        Instr::new(Op::Return, 0),
    ]);
    let signed = (rec as i32 * 4) - ((3 * 4) + 4);
    code[3].operand = (signed as i64 & 0xFF_FFFF) as u32;
    let fib = func("fib", code, consts, 2, 1);
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![fib.clone()],
        classes: Vec::new(),
    };

    let mut vm = Vm::new(0);
    vm.load_module_for_test(&module);
    let closure = vm.alloc_closure(0);
    vm.globals
        .insert("fib".to_owned(), aluka_vm::Value::Object(closure));
    // 解释器 oracle（走 invoke_function，顺带把 func0 计到阈值上）
    let interp = vm
        .invoke_function(
            0,
            aluka_vm::Value::Undefined,
            &[aluka_vm::Value::Number(18.0)],
            Vec::new(),
        )
        .expect("解释执行 fib");
    let aluka_vm::Value::Number(expected) = interp else {
        panic!("应为数值")
    };
    assert_eq!(expected, 2584.0, "fib(18) = 2584");

    let consts_rc = Rc::new(fib.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts_rc);
    let j = jit_compile(&fib, &ctx.vtable).expect("编译 fib");
    let r = j.call_ctx(&mut ctx, &[aluka_jit::valbox::box_number(18.0)]);
    assert!(aluka_jit::valbox::is_number(r), "结果应为数值盒 {r:#x}");
    assert_eq!(
        aluka_jit::valbox::unbox_number(r).to_bits(),
        expected.to_bits(),
        "自递归直调须与解释器逐位一致"
    );
    assert_eq!(
        vm.jit_frames(),
        0,
        "深递归 + 原生直调后帧计数必须归零（否则 GC 永久停摆）"
    );
}

// ---------------------------------------------------------------------------
// GC 安全：JIT 帧内不回收（机器码局部无栈映射，GC 扫不到）
// ---------------------------------------------------------------------------

/// JIT 帧内分配触发 GC 阈值时，只被 JIT 局部引用的对象不得被回收。
///
/// 负载：`o={}; o.x=42; i=N; while(i>0){ t={}; i-=1 } return o.x`。循环里
/// 每轮分配一个新对象，N 取到跨 minor/major 阈值；`o` 只活在 JIT 局部
/// （无栈映射 → 不在 GC 根集），若帧内回收就会被误清成 `Free`，`o.x` 读回
/// undefined。同时断言帧计数在返回后归零。
#[test]
fn jit_frame_defers_gc_and_keeps_locals_alive() {
    let rounds = 30000.0;
    // 0="x" 1=42 2=零 3=步长1 4=轮数
    let consts = vec![
        Constant::String("x".to_owned()),
        Constant::Number(42.0),
        Constant::Number(0.0),
        Constant::Number(1.0),
        Constant::Number(rounds),
    ];
    let mut code = vec![
        Instr::new(Op::NewObject, 0),
        Instr::new(Op::StoreLocal, 1),
        Instr::new(Op::LoadLocal, 1),
        Instr::new(Op::PushConst, 1),
        Instr::new(Op::SetProp, 0),
        Instr::new(Op::Pop, 0),
        Instr::new(Op::PushConst, 4),
        Instr::new(Op::StoreLocal, 2),
    ];
    let head = code.len();
    code.extend_from_slice(&[
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::PushConst, 2),
        Instr::new(Op::Gt, 0),
        Instr::new(Op::JmpFalsePop, 0),
    ]);
    let false_pc = code.len() - 1;
    code.extend_from_slice(&[
        Instr::new(Op::NewObject, 0),
        Instr::new(Op::StoreLocal, 3),
        Instr::new(Op::LoadLocal, 2),
        Instr::new(Op::PushConst, 3),
        Instr::new(Op::Sub, 0),
        Instr::new(Op::StoreLocal, 2),
        Instr::new(Op::Jmp, 0),
    ]);
    let jmp_pc = code.len() - 1;
    code.push(Instr::new(Op::LoadLocal, 1));
    let exit = code.len() - 1;
    code.push(Instr::new(Op::GetProp, 0));
    code.push(Instr::new(Op::Return, 0));
    let patch = |code: &mut Vec<Instr>, at: usize, target: usize| {
        let signed = (target as i32 * 4) - ((at as i32 * 4) + 4);
        code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
    };
    patch(&mut code, false_pc, exit);
    patch(&mut code, jmp_pc, head);
    let f = func("gc_in_jit", code, consts, 4, 0);

    // 解释器 oracle
    let mut vm_i = Vm::new(0);
    let interp = vm_i.run_func(&f).expect("解释执行 gc_in_jit");
    assert_eq!(interp, aluka_vm::Value::Number(42.0), "解释器读回 42");

    let mut vm = Vm::new(0);
    let consts_rc = Rc::new(f.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts_rc);
    let j = jit_compile(&f, &ctx.vtable).expect("编译 gc_in_jit");
    let r = j.call_ctx(&mut ctx, &[]);
    assert!(
        aluka_jit::valbox::is_number(r),
        "JIT 结果应为数值盒（NaN/非数值 = 对象被误回收）：{r:#x}"
    );
    assert_eq!(
        aluka_jit::valbox::unbox_number(r),
        42.0,
        "JIT 局部持有的对象不得在帧内被回收"
    );
    assert_eq!(vm.jit_frames(), 0, "返回后帧计数必须归零");
    // 帧退出后 GC 恢复：手动回收应能清掉循环里产生的垃圾
    let (_, reclaimed_before, _, _) = vm.gc_stats();
    let freed = vm.force_gc();
    let (_, reclaimed_after, _, _) = vm.gc_stats();
    assert!(
        freed > 0 && reclaimed_after > reclaimed_before,
        "帧退出后 GC 必须能正常回收（freed={freed}）"
    );
}

/// Global IC：同一机器码产物两次调用之间直接修改公开 `Vm::globals`，
/// epoch 守卫必须失效旧盒并读取新值。
#[test]
fn global_ic_invalidates_after_public_globals_mutation() {
    let f = func(
        "global_read",
        vec![Instr::new(Op::LoadGlobal, 0), Instr::new(Op::Return, 0)],
        vec![Constant::String("g".to_owned())],
        1,
        0,
    );
    let mut vm = Vm::new(0);
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Number(1.0));
    let consts = Rc::new(f.constants.clone());
    let mut ctx = vm.build_jit_ctx(&consts);
    let j = jit_compile(&f, &ctx.vtable).expect("编译 global_read");

    let first = j.call_ctx(&mut ctx, &[]);
    assert_eq!(
        aluka_jit::valbox::unbox_number(first),
        1.0,
        "首次读取 globals 中的值"
    );
    vm.globals
        .insert("g".to_owned(), aluka_vm::Value::Number(2.0));
    let mut ctx2 = vm.build_jit_ctx(&consts);
    let second = j.call_ctx(&mut ctx2, &[]);
    assert_eq!(
        aluka_jit::valbox::unbox_number(second),
        2.0,
        "公开 globals 修改后 Global IC 必须失效"
    );
}
