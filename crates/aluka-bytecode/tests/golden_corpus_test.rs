//! 黄金语料库（Golden Bytecode Corpus）106 条全指令覆盖率与解码集成测试。
//!
//! 该测试读取由 Go 编译器收割的真实 .bc 缓存模块，
//! 校验其格式合规性，并断言整个语料库对 106 条 ISA 字节码指令的 100% 全覆盖。

use aluka_bytecode::{Instr, Op};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// 从二进制数据中解码出全部函数的指令流。
fn decode_module_instructions(data: &[u8]) -> Result<Vec<Instr>, String> {
    if data.len() < 20 {
        return Err("文件长度不足".into());
    }
    if &data[0..8] != b"ALUKABC1" {
        return Err("无效的魔数头".into());
    }
    let version = u32::from_le_bytes(data[8..12].try_into().unwrap());
    if version != 30 {
        return Err(format!("版本不匹配: {version}"));
    }
    let func_count = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;

    let mut offset = 20;
    let mut instructions = Vec::new();

    let read_u32 = |o: &mut usize| -> Result<u32, String> {
        if *o + 4 > data.len() {
            return Err("读取 u32 越界".into());
        }
        let val = u32::from_le_bytes(data[*o..*o + 4].try_into().unwrap());
        *o += 4;
        Ok(val)
    };

    let read_string = |o: &mut usize| -> Result<(), String> {
        let len = read_u32(o)? as usize;
        if *o + len > data.len() {
            return Err("读取字符串越界".into());
        }
        *o += len;
        Ok(())
    };

    let read_uvarint = |o: &mut usize| -> Result<u64, String> {
        let mut x = 0u64;
        let mut s = 0u32;
        loop {
            if *o >= data.len() {
                return Err("读取 uvarint 越界".into());
            }
            let b = data[*o];
            *o += 1;
            if b < 0x80 {
                return Ok(x | ((b as u64) << s));
            }
            x |= ((b & 0x7f) as u64) << s;
            s += 7;
        }
    };

    let read_constants = |o: &mut usize| -> Result<(), String> {
        let count = read_u32(o)? as usize;
        for _ in 0..count {
            if *o >= data.len() {
                return Err("读取常量标签越界".into());
            }
            let tag = data[*o];
            *o += 1;
            match tag {
                1 => *o += 8, // Number float64
                2 | 3 => {
                    // String / BigInt (uvarint 长度)
                    let len = read_uvarint(o)? as usize;
                    if *o + len > data.len() {
                        return Err("读取常量字符串越界".into());
                    }
                    *o += len;
                }
                4 => *o += 1, // Bool
                5 => {}       // Null
                _ => return Err(format!("未知常量标签: {tag}")),
            }
        }
        Ok(())
    };

    for _ in 0..func_count {
        read_string(&mut offset)?; // Name
        if offset + 52 > data.len() {
            return Err("读取标量头越界".into());
        }
        let code_len =
            u32::from_le_bytes(data[offset + 24..offset + 28].try_into().unwrap()) as usize;
        offset += 52;

        if offset + code_len > data.len() {
            return Err("读取指令流越界".into());
        }
        let code_bytes = &data[offset..offset + code_len];
        offset += code_len;

        for chunk in code_bytes.chunks_exact(4) {
            let opcode_byte = chunk[0];
            let operand = ((chunk[1] as u32) << 16) | ((chunk[2] as u32) << 8) | (chunk[3] as u32);
            let op = Op::from_opcode(opcode_byte)
                .ok_or_else(|| format!("未识别的操作码: {opcode_byte}"))?;
            instructions.push(Instr::new(op, operand));
        }

        read_string(&mut offset)?; // SourceFile
        read_constants(&mut offset)?; // Constants

        let upvalue_count = read_u32(&mut offset)? as usize;
        offset += upvalue_count * 8; // Upvalues

        let has_native = read_u32(&mut offset)?;
        if has_native == 1 {
            let _param_count = read_u32(&mut offset)?;
            let instr_count = read_u32(&mut offset)? as usize;
            offset += instr_count * 8;
        }

        let try_count = read_u32(&mut offset)? as usize;
        offset += try_count * 32;

        let line_count = read_u32(&mut offset)? as usize;
        offset += line_count * 8;
    }

    Ok(instructions)
}

#[test]
fn test_golden_corpus_reaches_106_opcode_coverage() {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // aluka_r/crates/aluka-bytecode -> 往上两层到 aluka_r
    root.pop();
    root.pop();

    let corpus_dir = root.join("tests").join("golden").join("corpus");
    assert!(
        corpus_dir.exists(),
        "golden 语料目录必须存在: {:?}",
        corpus_dir
    );

    let entries = fs::read_dir(&corpus_dir).expect("读取 golden 语料目录失败");
    let mut covered_opcodes = HashSet::new();
    let mut total_instructions = 0;
    let mut module_count = 0;

    for entry in entries {
        let entry = entry.expect("读取目录条目失败");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("bc") {
            module_count += 1;
            let data = fs::read(&path).expect("读取 .bc 字节码文件失败");
            let instrs = decode_module_instructions(&data)
                .unwrap_or_else(|e| panic!("解析 {:?} 失败: {}", path, e));
            total_instructions += instrs.len();
            for instr in instrs {
                covered_opcodes.insert(instr.op.opcode());
            }
        }
    }

    println!(
        "Golden 语料测试统计: 扫描了 {} 个模块，共 {} 条指令，覆盖了 {}/106 种独立操作码",
        module_count,
        total_instructions,
        covered_opcodes.len()
    );

    assert!(
        module_count >= 20,
        "语料库模块数必须 >= 20，实际为: {}",
        module_count
    );

    for op in 0..=105u8 {
        assert!(
            covered_opcodes.contains(&op),
            "操作码 {} ({:?}) 必须在 golden 语料中被至少覆盖一次！",
            op,
            Op::from_opcode(op)
        );
    }

    assert_eq!(
        covered_opcodes.len(),
        106,
        "语料库必须达到 106/106 全指令覆盖！"
    );
}
