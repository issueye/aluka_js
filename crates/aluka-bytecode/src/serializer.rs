//! 二进制字节码序列化器（ALUKABC1 Version 30）。

use crate::verifier::{BytecodeModule, Constant};

fn write_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn write_uvarint(buf: &mut Vec<u8>, mut x: u64) {
    while x >= 0x80 {
        buf.push((x as u8 & 0x7f) | 0x80);
        x >>= 7;
    }
    buf.push(x as u8);
}

impl BytecodeModule {
    /// 将字节码模块序列化为 `ALUKABC1`（Version 30）标准二进制字节流。
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // 1. 20 字节容器头
        buf.extend_from_slice(b"ALUKABC1");
        write_u32(&mut buf, self.version);
        write_u32(&mut buf, self.functions.len() as u32);
        write_u32(&mut buf, self.classes.len() as u32);

        // 2. 函数模板列表
        for (i, func) in self.functions.iter().enumerate() {
            write_string(&mut buf, &func.name);

            // 52 字节标量头
            write_u32(&mut buf, func.num_params);
            write_u32(&mut buf, func.num_locals);
            write_u32(&mut buf, if func.is_var_args { 1 } else { 0 });
            write_u32(&mut buf, if func.is_generator { 1 } else { 0 });
            write_u32(&mut buf, if func.is_async { 1 } else { 0 });
            write_u32(&mut buf, if func.is_arrow { 1 } else { 0 });
            let code_len = (func.code.len() * 4) as u32;
            write_u32(&mut buf, code_len);
            // 20 字节标量字段 (arguments_slot, no_arguments_object, new_target_slot,
            // inlinable, nfe_slot)。模块未提供 header_extras 时回退旧默认
            // (arguments_slot: -1, no_arguments_object: true, ...)，输出逐字节不变。
            let (arguments_slot, no_arguments_object, new_target_slot, inlinable) = self
                .header_extras
                .get(i)
                .map(|e| {
                    (
                        e.arguments_slot,
                        e.no_arguments_object,
                        e.new_target_slot,
                        e.inlinable,
                    )
                })
                .unwrap_or((-1, true, -1, false));
            write_u32(&mut buf, arguments_slot as u32);
            write_u32(&mut buf, u32::from(no_arguments_object));
            write_u32(&mut buf, new_target_slot as u32);
            write_u32(&mut buf, u32::from(inlinable));
            write_u32(&mut buf, u32::MAX);
            write_u32(&mut buf, func.max_stack);

            // 写入指令流 (4 字节: 1 字节操作码 + 3 字节大端操作数)
            for instr in &func.code {
                buf.push(instr.op.opcode());
                buf.push(((instr.operand >> 16) & 0xFF) as u8);
                buf.push(((instr.operand >> 8) & 0xFF) as u8);
                buf.push((instr.operand & 0xFF) as u8);
            }

            write_string(&mut buf, &func.source_file);

            // 常量池
            write_u32(&mut buf, func.constants.len() as u32);
            for c in &func.constants {
                match c {
                    Constant::Number(f) => {
                        buf.push(1);
                        buf.extend_from_slice(&f.to_le_bytes());
                    }
                    Constant::String(s) => {
                        buf.push(2);
                        write_uvarint(&mut buf, s.len() as u64);
                        buf.extend_from_slice(s.as_bytes());
                    }
                    Constant::BigInt(s) => {
                        buf.push(3);
                        write_uvarint(&mut buf, s.len() as u64);
                        buf.extend_from_slice(s.as_bytes());
                    }
                    Constant::Bool(b) => {
                        buf.push(4);
                        buf.push(if *b { 1 } else { 0 });
                    }
                    Constant::Null => {
                        buf.push(5);
                    }
                }
            }

            // Upvalues
            write_u32(&mut buf, func.upvalues.len() as u32);
            for uv in &func.upvalues {
                write_u32(&mut buf, if uv.is_local { 1 } else { 0 });
                write_u32(&mut buf, uv.index);
            }

            // NativeCallback (0 表示无)
            write_u32(&mut buf, 0);

            // TryTable
            write_u32(&mut buf, func.try_table.len() as u32);
            for entry in &func.try_table {
                write_u32(&mut buf, entry.start_pc);
                write_u32(&mut buf, entry.catch_pc);
                write_u32(&mut buf, entry.finally_pc);
                write_u32(&mut buf, if entry.has_catch { 1 } else { 0 });
                write_u32(&mut buf, if entry.has_finally { 1 } else { 0 });
                write_u32(&mut buf, entry.end_pc);
                write_u32(&mut buf, entry.catch_end_pc);
                write_u32(&mut buf, entry.finally_end_pc);
            }

            // LineStarts (0 表示空)
            write_u32(&mut buf, 0);
        }

        // 3. 类模板列表
        for cls in &self.classes {
            write_string(&mut buf, &cls.name);

            // 16 字节标量头
            write_u32(&mut buf, if cls.has_super { 1 } else { 0 });
            write_u32(&mut buf, cls.constructor_index);
            write_u32(&mut buf, cls.methods.len() as u32);
            write_u32(&mut buf, cls.computed_indices.len() as u32);

            // 方法列表
            for m in &cls.methods {
                write_u32(&mut buf, m.func_index);
                write_u32(&mut buf, if m.is_static { 1 } else { 0 });
                write_u32(&mut buf, m.kind);
                write_string(&mut buf, &m.name);
            }

            // 计算属性索引列表
            for ci in &cls.computed_indices {
                write_u32(&mut buf, *ci);
            }
        }

        buf
    }
}
