//! ISA 发布契约容器（M4）：`.aluc` 二进制容器与 `.alua` 文本汇编。
//!
//! 规范见 `aluka_r/docs/isa-aluc-spec.md`。设计要点：
//! - **内外分层**：`.aluc` 容器（ALUKACC1，容器版本独立演进）内嵌完整
//!   ALUKABC1（Version 30）payload——ISA 语义版本由内层承载，容器布局与
//!   ISA 语义解耦；
//! - **调试段可剥离**：`--strip-debug` 产出无调试段容器（分发）；
//! - [`BytecodeModule::load_any`] 按魔数嗅探，aluvm 对两种格式透明。

use crate::verifier::{BytecodeModule, VerifyError};
use std::ops::Range;

/// `.aluc` 容器魔数。
pub const ALUC_MAGIC: &[u8; 8] = b"ALUKACC1";
/// `.aluc` 容器版本（容器布局变更才递增，ISA 语义版本由内层 payload 承载）。
pub const ALUC_CONTAINER_VERSION: u32 = 1;
/// flags：bit0 = 调试段存在。
const FLAG_HAS_DEBUG: u32 = 0x1;

impl BytecodeModule {
    /// 序列化为 `.aluc` 发布容器。`strip_debug = true` 时不写调试段。
    #[must_use]
    pub fn serialize_aluc(&self, strip_debug: bool) -> Vec<u8> {
        let payload = self.serialize();
        let mut buf = Vec::new();
        buf.extend_from_slice(ALUC_MAGIC);
        buf.extend_from_slice(&ALUC_CONTAINER_VERSION.to_le_bytes());
        let flags: u32 = if strip_debug { 0 } else { FLAG_HAS_DEBUG };
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        let debug = if strip_debug {
            Vec::new()
        } else {
            self.debug_section()
        };
        buf.extend_from_slice(&(debug.len() as u32).to_le_bytes());
        buf.extend_from_slice(&payload);
        buf.extend_from_slice(&debug);
        buf
    }

    /// 调试段：源文件路径 + 函数调试名表。
    fn debug_section(&self) -> Vec<u8> {
        let mut d = Vec::new();
        write_str(&mut d, &self.debug_source_file());
        d.extend_from_slice(&(self.functions.len() as u32).to_le_bytes());
        for f in &self.functions {
            write_str(&mut d, &f.name);
        }
        d
    }

    /// 调试段登记的源文件路径（无则空串）。
    #[must_use]
    pub fn debug_source_file(&self) -> String {
        self.functions
            .first()
            .map(|f| f.source_file.clone())
            .unwrap_or_default()
    }

    /// 解析 `.aluc` 容器：返回模块与内层 payload 的字节范围（供
    /// [`crate::read_all_func_header_extras`] 读取函数扩展标量头）。
    ///
    /// # Errors
    /// 魔数/版本不符、payload 解析失败时返回 [`VerifyError`]。
    pub fn deserialize_aluc(data: &[u8]) -> Result<(Self, Range<usize>), VerifyError> {
        if data.len() < 24 {
            return Err(VerifyError::UnexpectedEof("ALUKACC1 容器头过短"));
        }
        if &data[0..8] != ALUC_MAGIC {
            return Err(VerifyError::V1BadMagic);
        }
        let container_version = u32::from_le_bytes(data[8..12].try_into().unwrap());
        if container_version != ALUC_CONTAINER_VERSION {
            return Err(VerifyError::V2BadVersion(container_version));
        }
        let flags = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let payload_len = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
        let debug_len = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
        if flags & FLAG_HAS_DEBUG == 0 && debug_len != 0 {
            return Err(VerifyError::UnexpectedEof(
                "ALUKACC1 flags 无调试段但 debug_len 非零",
            ));
        }
        let payload_start: usize = 24;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or(VerifyError::UnexpectedEof("ALUKACC1 payload 长度溢出"))?;
        if data.len() < payload_end + debug_len {
            return Err(VerifyError::UnexpectedEof("ALUKACC1 容器不完整"));
        }
        let module = Self::deserialize(&data[payload_start..payload_end])?;
        Ok((module, payload_start..payload_end))
    }

    /// 加载入口：按魔数嗅探 `.aluc`（ALUKACC1）或标准二进制格式（ALUKABC1），
    /// 返回模块与「函数扩展标量头」所在的字节切片范围。
    ///
    /// # Errors
    /// 两种魔数均不匹配或解析失败时返回 [`VerifyError`]。
    pub fn load_any_container(data: &[u8]) -> Result<(Self, Range<usize>), VerifyError> {
        if data.len() >= 8 && &data[0..8] == ALUC_MAGIC {
            let (module, range) = Self::deserialize_aluc(data)?;
            return Ok((module, range));
        }
        let module = Self::deserialize(data)?;
        Ok((module, 0..data.len()))
    }

    /// `.alua` 文本汇编转储（确定性文本，规范见 `docs/isa-aluc-spec.md`）。
    #[must_use]
    pub fn write_alua(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(".module {}\n", self.debug_source_file()));
        let isa_version = self.version;
        out.push_str(&format!(".version {isa_version}\n"));
        for f in &self.functions {
            out.push_str(&format!(
                ".func {} params={} locals={} varargs={} generator={} async={}\n",
                f.name,
                f.num_params,
                f.num_locals,
                f.is_var_args as u8,
                f.is_generator as u8,
                f.is_async as u8
            ));
            for (i, c) in f.constants.iter().enumerate() {
                match c {
                    crate::Constant::Number(n) => {
                        out.push_str(&format!(".const {i} num {n}\n"));
                    }
                    crate::Constant::String(s) => {
                        out.push_str(&format!(".const {i} str {}\n", alua_escape(s)));
                    }
                    crate::Constant::BigInt(t) => {
                        out.push_str(&format!(".const {i} bigint {t}\n"));
                    }
                    crate::Constant::Bool(b) => {
                        out.push_str(&format!(".const {i} bool {b}\n"));
                    }
                    crate::Constant::Null => {
                        out.push_str(&format!(".const {i} null\n"));
                    }
                }
            }
            for (pc, instr) in f.code.iter().enumerate() {
                out.push_str(&format!(
                    "  {pc}: {} 0x{:06x}\n",
                    instr.op.name(),
                    instr.operand
                ));
            }
            for e in &f.try_table {
                out.push_str(&format!(
                    ".try start={} catch={} finally={} has_catch={} has_finally={} end={} catch_end={} finally_end={}\n",
                    e.start_pc, e.catch_pc, e.finally_pc, e.has_catch as u8,
                    e.has_finally as u8, e.end_pc, e.catch_end_pc, e.finally_end_pc
                ));
            }
            out.push_str(".endfunc\n");
        }
        for c in &self.classes {
            out.push_str(&format!(
                ".class {} super={} ctor={} methods={} computed={}\n",
                c.name,
                c.has_super as u8,
                c.constructor_index,
                c.methods.len(),
                c.computed_indices.len()
            ));
        }
        out
    }
}

/// 写 u32 长度前缀字符串。
fn write_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// `.alua` 字符串转义（换行/引号/反斜杠）。
fn alua_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{Instr, Op};
    use crate::{Constant, FuncTemplate};

    /// 最小模块（单函数：PushConst → Return）。
    fn minimal_module() -> BytecodeModule {
        BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![FuncTemplate {
                name: "main".to_owned(),
                num_params: 0,
                num_locals: 1,
                is_var_args: false,
                is_generator: false,
                is_async: false,
                is_arrow: false,
                code: vec![Instr::new(Op::PushConst, 0), Instr::new(Op::Return, 0)],
                max_stack: 8,
                source_file: "t.js".to_owned(),
                constants: vec![Constant::Number(7.0)],
                upvalues: Vec::new(),
                try_table: Vec::new(),
            }],
            classes: Vec::new(),
        }
    }

    /// 容器 roundtrip：serialize_aluc → deserialize_aluc 还原等价模块，
    /// 且返回的内层 payload 范围以 ALUKABC1 魔数开头。
    #[test]
    fn aluc_roundtrip_preserves_module() {
        let m = minimal_module();
        let bytes = m.serialize_aluc(false);
        assert_eq!(&bytes[0..8], b"ALUKACC1");
        let (back, payload) = BytecodeModule::deserialize_aluc(&bytes).expect("roundtrip");
        assert_eq!(back.version, m.version);
        assert_eq!(back.functions.len(), 1);
        assert_eq!(back.functions[0].name, "main");
        assert_eq!(&bytes[payload.start..payload.start + 8], b"ALUKABC1");
    }

    /// load_any 嗅探：两种魔数均可加载。
    #[test]
    fn load_any_sniffs_both_magics() {
        let m = minimal_module();
        let bc_bytes = m.serialize();
        let (back_bc, _) = BytecodeModule::load_any_container(&bc_bytes).expect("alukabc1");
        assert_eq!(back_bc.functions[0].name, "main");
        let aluc = m.serialize_aluc(true);
        let (back_aluc, _) = BytecodeModule::load_any_container(&aluc).expect("aluc");
        assert_eq!(back_aluc.functions[0].name, "main");
    }

    /// 剥离选项：strip_debug 容器 flags bit0=0 且无调试段。
    #[test]
    fn strip_debug_drops_debug_section() {
        let m = minimal_module();
        let full = m.serialize_aluc(false);
        let stripped = m.serialize_aluc(true);
        let flags_full = u32::from_le_bytes(full[12..16].try_into().unwrap());
        let flags_strip = u32::from_le_bytes(stripped[12..16].try_into().unwrap());
        assert_eq!(flags_full & 1, 1, "完整容器应有调试段");
        assert_eq!(flags_strip & 1, 0, "剥离容器无调试段");
        let debug_len_full = u32::from_le_bytes(full[20..24].try_into().unwrap());
        let debug_len_strip = u32::from_le_bytes(stripped[20..24].try_into().unwrap());
        assert!(debug_len_full > 0);
        assert_eq!(debug_len_strip, 0);
        assert!(stripped.len() < full.len());
    }

    /// .alua 文本汇编：确定性 + 关键字段齐全。
    #[test]
    fn alua_dump_is_deterministic_and_complete() {
        let m = minimal_module();
        let a = m.write_alua();
        let b = m.write_alua();
        assert_eq!(a, b, "转储必须确定性");
        assert!(a.contains(".func main params=0"));
        assert!(a.contains("0: PUSH_CONST 0x000000"));
        assert!(a.contains("1: RETURN 0x000000"));
        assert!(a.contains(".endfunc"));
    }
}
