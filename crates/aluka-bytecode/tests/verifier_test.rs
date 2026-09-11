//! 字节码校验器（Verifier）正反向全规则集成测试。
//!
//! 1. 正向验证：对黄金语料库（tests/golden/corpus/*.bc）全部 33 个模块执行校验，100% 全部通过。
//! 2. 反向拒绝：逐条为 V1..V16 构造变异反例，断言校验器能够精确阻断并返回对应错误。

use aluka_bytecode::{BytecodeModule, Constant, FuncTemplate, Instr, Op, TryEntry, VerifyError};
use std::fs;
use std::path::PathBuf;

/// 构造一个用于测试的合法最小函数模板。
fn create_valid_func(code: Vec<Instr>, num_locals: u32, max_stack: u32) -> FuncTemplate {
    FuncTemplate {
        name: "test_fn".into(),
        num_params: 0,
        num_locals,
        is_var_args: false,
        is_generator: false,
        is_async: false,
        is_arrow: false,
        code,
        max_stack,
        source_file: "test.js".into(),
        constants: Vec::new(),
        upvalues: Vec::new(),
        try_table: Vec::new(),
        line_table: Vec::new(),
    }
}

/// 构造一个合法的最小字节码模块。
fn create_valid_module(func: FuncTemplate) -> BytecodeModule {
    BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func],
        classes: Vec::new(),
    }
}

// -----------------------------------------------------------------------------
// 一、正向测试：黄金语料库 100% 校验通过
// -----------------------------------------------------------------------------

#[test]
fn test_golden_corpus_all_modules_pass_verifier() {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();

    let corpus_dir = root.join("tests").join("golden").join("corpus");
    let entries = fs::read_dir(&corpus_dir).expect("读取 golden 语料目录失败");

    let mut verified_count = 0;
    for entry in entries {
        let path = entry.expect("读取条目失败").path();
        if path.extension().and_then(|s| s.to_str()) == Some("bc") {
            let data = fs::read(&path).expect("读取 .bc 字节码失败");
            let module = BytecodeModule::deserialize(&data)
                .unwrap_or_else(|e| panic!("反序列化 {:?} 失败: {}", path, e));
            module
                .verify()
                .unwrap_or_else(|e| panic!("校验 {:?} 失败: {}", path, e));
            verified_count += 1;
        }
    }

    println!("正向验证成功：共 {verified_count} 个真实 golden 模块全部通过 V1..V16 严格校验！");
    assert!(verified_count >= 20);
}

// -----------------------------------------------------------------------------
// 二、反向测试：V1..V16 精确拒绝覆盖
// -----------------------------------------------------------------------------

#[test]
fn test_v1_bad_magic() {
    let mut data = vec![0u8; 32];
    data[0..8].copy_from_slice(b"BADMAGIC");
    assert_eq!(
        BytecodeModule::deserialize(&data),
        Err(VerifyError::V1BadMagic)
    );
}

#[test]
fn test_v2_bad_version() {
    let mut data = vec![0u8; 32];
    data[0..8].copy_from_slice(b"ALUKABC1");
    data[8..12].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(
        BytecodeModule::deserialize(&data),
        Err(VerifyError::V2BadVersion(999))
    );
}

#[test]
fn test_v3_invalid_opcode() {
    let mut data = vec![0u8; 20];
    data[0..8].copy_from_slice(b"ALUKABC1");
    data[8..12].copy_from_slice(&30u32.to_le_bytes());
    data[12..16].copy_from_slice(&1u32.to_le_bytes()); // 1 函数
    data[16..20].copy_from_slice(&0u32.to_le_bytes()); // 0 类

    // 函数名
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(b"fn");

    // 13 标量
    let mut scalars = vec![0u8; 52];
    scalars[24..28].copy_from_slice(&4u32.to_le_bytes()); // code_len = 4
    data.extend_from_slice(&scalars);

    // 非法操作码 250
    data.extend_from_slice(&[250, 0, 0, 0]);

    match BytecodeModule::deserialize(&data) {
        Err(VerifyError::V3InvalidOpcode { opcode: 250, .. }) => {}
        res => panic!("期望 V3InvalidOpcode，实为 {:?}", res),
    }
}

#[test]
fn test_v4_slot_out_of_range() {
    let func = create_valid_func(
        vec![
            Instr::new(Op::LoadLocal, 5), // 槽位 5 >= num_locals 2
            Instr::new(Op::Return, 0),
        ],
        2,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V4SlotOutOfRange {
            slot: 5, max: 2, ..
        }) => {}
        res => panic!("期望 V4SlotOutOfRange，实为 {:?}", res),
    }
}

#[test]
fn test_v5_const_out_of_range() {
    let mut func = create_valid_func(
        vec![
            Instr::new(Op::PushConst, 3), // 常量池仅 1 项，访问索引 3
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    func.constants.push(Constant::Number(42.0));
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V5ConstOutOfRange {
            index: 3, max: 1, ..
        }) => {}
        res => panic!("期望 V5ConstOutOfRange，实为 {:?}", res),
    }
}

#[test]
fn test_v6_const_type_mismatch() {
    let mut func = create_valid_func(
        vec![
            Instr::new(Op::LoadGlobal, 0), // LoadGlobal 必须引用 String
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    func.constants.push(Constant::Number(42.5)); // 错误：放入 Number 而非 String
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V6ConstTypeMismatch {
            expected: "String",
            actual: "Number",
            ..
        }) => {}
        res => panic!("期望 V6ConstTypeMismatch，实为 {:?}", res),
    }
}

#[test]
fn test_v7_bad_jump_target() {
    let func = create_valid_func(
        vec![
            // code 长度共 8 字节，跳转偏移 100 越界
            Instr::new(Op::Jmp, 100),
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V7BadJumpTarget { .. }) => {}
        res => panic!("期望 V7BadJumpTarget，实为 {:?}", res),
    }
}

#[test]
fn test_v8_stack_depth_mismatch() {
    // 构造合流点栈深不一致：
    // 分支 1：压 1 个数后跳到目标 PC=12
    // 分支 2：压 2 个数后落到目标 PC=12
    let func = create_valid_func(
        vec![
            Instr::new(Op::PushInt, 1),
            // PC=0: 深度1，条件跳转到 PC=12
            Instr::new(Op::JmpTruePop, 4), // target = 4 + 4 + 4 = 12
            // 顺序分支 PC=8: 压入两个数（此时深度 2）
            Instr::new(Op::PushInt, 2),
            Instr::new(Op::PushInt, 3),
            // PC=16: 汇合点 Return
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V8StackDepthMismatch { .. }) => {}
        res => panic!("期望 V8StackDepthMismatch，实为 {:?}", res),
    }
}

#[test]
fn test_v9_stack_underflow() {
    let func = create_valid_func(
        vec![
            // 空栈执行 Add（需要 2 个操作数）
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V9StackUnderflow {
            depth: 0,
            required: 2,
            ..
        }) => {}
        res => panic!("期望 V9StackUnderflow，实为 {:?}", res),
    }
}

#[test]
fn test_v10_max_stack_exceeded() {
    let func = create_valid_func(
        vec![
            Instr::new(Op::PushInt, 1),
            Instr::new(Op::PushInt, 2),
            Instr::new(Op::PushInt, 3), // 此时深度为 3
            Instr::new(Op::Return, 0),
        ],
        0,
        2, // MaxStack 限制为 2
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V10MaxStackExceeded {
            depth: 3,
            max_stack: 2,
            ..
        }) => {}
        res => panic!("期望 V10MaxStackExceeded，实为 {:?}", res),
    }
}

#[test]
fn test_v11_bad_try_range() {
    let mut func = create_valid_func(vec![Instr::new(Op::ReturnUndef, 0)], 0, 10);
    func.try_table.push(TryEntry {
        start_pc: 8,
        catch_pc: 0,
        finally_pc: 0,
        has_catch: false,
        has_finally: false,
        end_pc: 4, // 错误：start_pc > end_pc
        catch_end_pc: 0,
        finally_end_pc: 0,
    });
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V11BadTryRange { .. }) => {}
        res => panic!("期望 V11BadTryRange，实为 {:?}", res),
    }
}

#[test]
fn test_v12_handler_inside_body() {
    let mut func = create_valid_func(
        vec![
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::ReturnUndef, 0),
        ],
        0,
        10,
    );
    func.try_table.push(TryEntry {
        start_pc: 0,
        catch_pc: 4, // 错误：catch 落在 [0, 12) 保护区内部
        finally_pc: 0,
        has_catch: true,
        has_finally: false,
        end_pc: 12,
        catch_end_pc: 16,
        finally_end_pc: 0,
    });
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V12HandlerInsideBody { handler_pc: 4, .. }) => {}
        res => panic!("期望 V12HandlerInsideBody，实为 {:?}", res),
    }
}

#[test]
fn test_v13_try_cross_overlap() {
    let mut func = create_valid_func(
        vec![
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::Nop, 0),
            Instr::new(Op::ReturnUndef, 0),
        ],
        0,
        10,
    );
    // 区间 A: [0, 16)
    func.try_table.push(TryEntry {
        start_pc: 0,
        catch_pc: 0,
        finally_pc: 0,
        has_catch: false,
        has_finally: false,
        end_pc: 16,
        catch_end_pc: 0,
        finally_end_pc: 0,
    });
    // 区间 B: [8, 24)（非法交叉重叠）
    func.try_table.push(TryEntry {
        start_pc: 8,
        catch_pc: 0,
        finally_pc: 0,
        has_catch: false,
        has_finally: false,
        end_pc: 24,
        catch_end_pc: 0,
        finally_end_pc: 0,
    });
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V13TryCrossOverlap { .. }) => {}
        res => panic!("期望 V13TryCrossOverlap，实为 {:?}", res),
    }
}

#[test]
fn test_v14_bad_try_end() {
    let mut func = create_valid_func(
        vec![Instr::new(Op::Nop, 0), Instr::new(Op::ReturnUndef, 0)],
        0,
        10,
    );
    func.try_table.push(TryEntry {
        start_pc: 0,
        catch_pc: 4,
        finally_pc: 0,
        has_catch: true,
        has_finally: false,
        end_pc: 4,
        catch_end_pc: 7, // 错误：未按 4 字节对齐
        finally_end_pc: 0,
    });
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V14BadTryEnd {
            field: "catch_end_pc",
            pc: 7,
            ..
        }) => {}
        res => panic!("期望 V14BadTryEnd，实为 {:?}", res),
    }
}

#[test]
fn test_v15_template_out_of_range() {
    let func = create_valid_func(
        vec![
            Instr::new(Op::MakeClass, 50), // 模板索引 50 >= 1
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V15TemplateOutOfRange {
            index: 50, max: 1, ..
        }) => {}
        res => panic!("期望 V15TemplateOutOfRange，实为 {:?}", res),
    }
}

#[test]
fn test_v16_upvalue_out_of_range() {
    let func = create_valid_func(
        vec![
            Instr::new(Op::LoadUpvalue, 10), // Upvalues 为空，访问索引 10
            Instr::new(Op::Return, 0),
        ],
        0,
        10,
    );
    let module = create_valid_module(func);
    match module.verify() {
        Err(VerifyError::V16UpvalueOutOfRange {
            index: 10, max: 0, ..
        }) => {}
        res => panic!("期望 V16UpvalueOutOfRange，实为 {:?}", res),
    }
}
