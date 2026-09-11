//! 字节码校验器（Verifier）：通过即安全（Valid implies Safe）硬契约。
//!
//! 强制执行 ISA 规范中的 V1..V16 全部安全校验规则。
//! 全面覆盖 Go 侧缺失的跨块栈深合流一致性、栈下溢防范、Try 表严格嵌套隔离、
//! 常量池类型匹配等重要约束，确保字节码在进入执行引擎前即消除内存安全隐患。

#![allow(missing_docs)]

use crate::op::{Instr, Op, OperandKind};
use std::collections::VecDeque;

/// 常量池条目。
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    /// 64位浮点数 (Tag 1)
    Number(f64),
    /// UTF-8 字符串 (Tag 2)
    String(String),
    /// 十进制大数字符串 (Tag 3)
    BigInt(String),
    /// 布尔值 (Tag 4)
    Bool(bool),
    /// 空值 (Tag 5)
    Null,
}

impl Constant {
    /// 常量类型的简要中文名称。
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Constant::Number(_) => "Number",
            Constant::String(_) => "String",
            Constant::BigInt(_) => "BigInt",
            Constant::Bool(_) => "Bool",
            Constant::Null => "Null",
        }
    }
}

/// 闭包上值捕获描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpvalueCapture {
    /// 在外层函数的局部槽位下标或外层上值下标
    pub index: u32,
    /// 是否捕获自外层局部槽位（true 为局部，false 为外层上值）
    pub is_local: bool,
}

/// Try 异常处理表项（32 字节对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryEntry {
    /// 保护区起始指令字节偏移
    pub start_pc: u32,
    /// Catch 处理块起始偏移（0 表示无 Catch）
    pub catch_pc: u32,
    /// Finally 处理块起始偏移（0 表示无 Finally）
    pub finally_pc: u32,
    /// 是否有 Catch 块
    pub has_catch: bool,
    /// 是否有 Finally 块
    pub has_finally: bool,
    /// 保护区结束指令字节偏移（开区间）
    pub end_pc: u32,
    /// Catch 处理块结束偏移
    pub catch_end_pc: u32,
    /// Finally 处理块结束偏移
    pub finally_end_pc: u32,
}

/// 函数模板。
#[derive(Debug, Clone, PartialEq)]
pub struct FuncTemplate {
    /// 函数名称
    pub name: String,
    /// 形式参数数量
    pub num_params: u32,
    /// 局部槽位总数
    pub num_locals: u32,
    /// 是否为变长参数
    pub is_var_args: bool,
    /// 是否为生成器函数
    pub is_generator: bool,
    /// 是否为异步函数
    pub is_async: bool,
    /// 是否为箭头函数
    pub is_arrow: bool,
    /// 指令流
    pub code: Vec<Instr>,
    /// 操作数栈峰值限制
    pub max_stack: u32,
    /// 源文件路径
    pub source_file: String,
    /// 常量池
    pub constants: Vec<Constant>,
    /// 上值捕获表
    pub upvalues: Vec<UpvalueCapture>,
    /// 异常处理表
    pub try_table: Vec<TryEntry>,
    /// LCOV 行覆盖表：(指令索引, 源码行号)。仅进程内编译且启用覆盖时非空；
    /// **不参与序列化**——`.bc` 分发形态无覆盖信息（登记：`aluka test` 进程内
    /// 编译路径才可用行覆盖）。
    pub line_table: Vec<(u32, u32)>,
}

/// 类方法定义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassMethod {
    /// 方法名
    pub name: String,
    /// 对应的函数模板索引
    pub func_index: u32,
    /// 是否为静态方法
    pub is_static: bool,
    /// 方法类型（普通、Getter、Setter 等）
    pub kind: u32,
}

/// 类模板。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassTemplate {
    /// 类名
    pub name: String,
    /// 是否有父类
    pub has_super: bool,
    /// 构造函数模板索引
    pub constructor_index: u32,
    /// 方法列表
    pub methods: Vec<ClassMethod>,
    /// 计算属性索引列表
    pub computed_indices: Vec<u32>,
}

/// 完整的字节码编译模块。
#[derive(Debug, Clone, PartialEq)]
pub struct BytecodeModule {
    /// 容器版本号
    pub version: u32,
    /// 函数模板列表
    pub functions: Vec<FuncTemplate>,
    /// 类模板列表
    pub classes: Vec<ClassTemplate>,
    /// 函数扩展标量头（与 `functions` 平行；空 = 全部使用「无 arguments」默认，
    /// 序列化输出与旧布局逐字节一致）
    pub header_extras: Vec<crate::extras::FuncHeaderExtras>,
}

/// 字节码校验失败的明确原因（对应 V1..V16 规则）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// V1: 魔数头不匹配
    V1BadMagic,
    /// V2: 格式版本不匹配
    V2BadVersion(u32),
    /// V3: 指令流未按 4 字节对齐
    V3UnalignedCode { func: String, len: usize },
    /// V3: 发现未定义的操作码数值
    V3InvalidOpcode { func: String, pc: usize, opcode: u8 },
    /// V4: 局部槽位操作数越界
    V4SlotOutOfRange {
        func: String,
        pc: usize,
        slot: u32,
        max: u32,
    },
    /// V5: 常量池索引越界
    V5ConstOutOfRange {
        func: String,
        pc: usize,
        index: u32,
        max: usize,
    },
    /// V6: 常量池条目类型不匹配
    V6ConstTypeMismatch {
        func: String,
        pc: usize,
        expected: &'static str,
        actual: &'static str,
    },
    /// V7: 相对跳转目标越界或未对齐
    V7BadJumpTarget {
        func: String,
        pc: usize,
        target_pc: i32,
    },
    /// V8: 控制流汇合点栈深度不一致
    V8StackDepthMismatch {
        func: String,
        pc: usize,
        expected: i32,
        actual: i32,
    },
    /// V9: 指令执行前操作数栈下溢
    V9StackUnderflow {
        func: String,
        pc: usize,
        depth: i32,
        required: u8,
    },
    /// V10: 操作数栈推导深度超出最大栈限制 MaxStack
    V10MaxStackExceeded {
        func: String,
        pc: usize,
        depth: i32,
        max_stack: u32,
    },
    /// V11: Try 区间无效（start_pc >= end_pc 或未对齐/越界）
    V11BadTryRange {
        func: String,
        start_pc: u32,
        end_pc: u32,
    },
    /// V12: 异常处理句柄（Catch/Finally）位于保护区内部
    V12HandlerInsideBody {
        func: String,
        handler_pc: u32,
        start_pc: u32,
        end_pc: u32,
    },
    /// V13: Try 区间存在非法的交叉重叠（必须严格包含或不相交）
    V13TryCrossOverlap {
        func: String,
        range_a: (u32, u32),
        range_b: (u32, u32),
    },
    /// V14: Try 边界结束偏移越界或未按 4 字节对齐
    V14BadTryEnd {
        func: String,
        field: &'static str,
        pc: u32,
    },
    /// V15: 模板索引超出模块有效范围
    V15TemplateOutOfRange {
        func: String,
        pc: usize,
        index: u32,
        max: usize,
    },
    /// V15: Try 表索引超出函数 TryTable 范围
    V15TryOutOfRange {
        func: String,
        pc: usize,
        index: u32,
        max: usize,
    },
    /// V16: 上值索引超出函数 Upvalues 范围
    V16UpvalueOutOfRange {
        func: String,
        pc: usize,
        index: u32,
        max: usize,
    },
    /// 数据损坏或过早 EOF
    UnexpectedEof(&'static str),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::V1BadMagic => write!(f, "V1: 容器魔数错误，必须为 ALUKABC1"),
            Self::V2BadVersion(v) => write!(f, "V2: 格式版本不匹配: {v}"),
            Self::V3UnalignedCode { func, len } => {
                write!(f, "V3: 函数 {func} 指令长度 {len} 未按 4 字节对齐")
            }
            Self::V3InvalidOpcode { func, pc, opcode } => {
                write!(f, "V3: 函数 {func} 偏移 {pc} 处发现未知操作码 {opcode}")
            }
            Self::V4SlotOutOfRange {
                func,
                pc,
                slot,
                max,
            } => write!(f, "V4: 函数 {func} 偏移 {pc} 局部槽位越界: {slot} >= {max}"),
            Self::V5ConstOutOfRange {
                func,
                pc,
                index,
                max,
            } => write!(
                f,
                "V5: 函数 {func} 偏移 {pc} 常量池索引越界: {index} >= {max}"
            ),
            Self::V6ConstTypeMismatch {
                func,
                pc,
                expected,
                actual,
            } => write!(
                f,
                "V6: 函数 {func} 偏移 {pc} 常量类型不匹配: 期望 {expected}, 实为 {actual}"
            ),
            Self::V7BadJumpTarget {
                func,
                pc,
                target_pc,
            } => write!(f, "V7: 函数 {func} 偏移 {pc} 跳转目标非法: {target_pc}"),
            Self::V8StackDepthMismatch {
                func,
                pc,
                expected,
                actual,
            } => write!(
                f,
                "V8: 函数 {func} 汇合点 {pc} 栈深不一致: 期望 {expected}, 实为 {actual}"
            ),
            Self::V9StackUnderflow {
                func,
                pc,
                depth,
                required,
            } => write!(
                f,
                "V9: 函数 {func} 偏移 {pc} 栈下溢: 当前深度 {depth} < 所需 {required}"
            ),
            Self::V10MaxStackExceeded {
                func,
                pc,
                depth,
                max_stack,
            } => write!(
                f,
                "V10: 函数 {func} 偏移 {pc} 栈深 {depth} 超出限制 MaxStack={max_stack}"
            ),
            Self::V11BadTryRange {
                func,
                start_pc,
                end_pc,
            } => write!(f, "V11: 函数 {func} Try 区间非法: [{start_pc}, {end_pc})"),
            Self::V12HandlerInsideBody {
                func,
                handler_pc,
                start_pc,
                end_pc,
            } => write!(
                f,
                "V12: 函数 {func} 异常处理器 {handler_pc} 位于保护区 [{start_pc}, {end_pc}) 内部"
            ),
            Self::V13TryCrossOverlap {
                func,
                range_a,
                range_b,
            } => write!(
                f,
                "V13: 函数 {func} Try 区间存在交叉重叠: {:?} 与 {:?}",
                range_a, range_b
            ),
            Self::V14BadTryEnd { func, field, pc } => {
                write!(f, "V14: 函数 {func} 边界字段 {field} 非法: {pc}")
            }
            Self::V15TemplateOutOfRange {
                func,
                pc,
                index,
                max,
            } => write!(
                f,
                "V15: 函数 {func} 偏移 {pc} 模板索引越界: {index} >= {max}"
            ),
            Self::V15TryOutOfRange {
                func,
                pc,
                index,
                max,
            } => write!(
                f,
                "V15: 函数 {func} 偏移 {pc} Try 表索引越界: {index} >= {max}"
            ),
            Self::V16UpvalueOutOfRange {
                func,
                pc,
                index,
                max,
            } => write!(
                f,
                "V16: 函数 {func} 偏移 {pc} 上值索引越界: {index} >= {max}"
            ),
            Self::UnexpectedEof(ctx) => write!(f, "数据过早截断: {ctx}"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl BytecodeModule {
    /// 从 `ALUKABC1`（Version 30）标准二进制反序列化为字节码模块。
    pub fn deserialize(data: &[u8]) -> Result<Self, VerifyError> {
        if data.len() < 20 {
            return Err(VerifyError::UnexpectedEof("文件头过短"));
        }
        // V1: 魔数
        if &data[0..8] != b"ALUKABC1" {
            return Err(VerifyError::V1BadMagic);
        }
        // V2: 版本
        let version = u32::from_le_bytes(data[8..12].try_into().unwrap());
        if version != 30 {
            return Err(VerifyError::V2BadVersion(version));
        }

        let func_count = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
        let class_count = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;

        let mut offset = 20;

        let read_u32 = |o: &mut usize| -> Result<u32, VerifyError> {
            if *o + 4 > data.len() {
                return Err(VerifyError::UnexpectedEof("读取 u32"));
            }
            let val = u32::from_le_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(val)
        };

        let read_string = |o: &mut usize| -> Result<String, VerifyError> {
            let len = read_u32(o)? as usize;
            if *o + len > data.len() {
                return Err(VerifyError::UnexpectedEof("读取字符串"));
            }
            let s = String::from_utf8_lossy(&data[*o..*o + len]).to_string();
            *o += len;
            Ok(s)
        };

        let read_uvarint = |o: &mut usize| -> Result<u64, VerifyError> {
            let mut x = 0u64;
            let mut s = 0u32;
            loop {
                if *o >= data.len() {
                    return Err(VerifyError::UnexpectedEof("读取 uvarint"));
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

        let mut functions = Vec::with_capacity(func_count);
        let mut header_extras = Vec::with_capacity(func_count);

        for _ in 0..func_count {
            let name = read_string(&mut offset)?;
            if offset + 52 > data.len() {
                return Err(VerifyError::UnexpectedEof("函数标量头"));
            }

            let num_params = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            let num_locals = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap());
            let is_var_args =
                u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) != 0;
            let is_generator =
                u32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap()) != 0;
            let is_async =
                u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap()) != 0;
            let is_arrow =
                u32::from_le_bytes(data[offset + 20..offset + 24].try_into().unwrap()) != 0;
            let code_len =
                u32::from_le_bytes(data[offset + 24..offset + 28].try_into().unwrap()) as usize;
            // 20 字节扩展标量：arguments_slot / no_arguments_object / new_target_slot /
            // inlinable / nfe_slot（回读以还原 BytecodeModule.header_extras）
            let arguments_slot =
                i32::from_le_bytes(data[offset + 28..offset + 32].try_into().unwrap());
            let no_arguments_object =
                u32::from_le_bytes(data[offset + 32..offset + 36].try_into().unwrap()) != 0;
            let new_target_slot =
                i32::from_le_bytes(data[offset + 36..offset + 40].try_into().unwrap());
            let inlinable =
                u32::from_le_bytes(data[offset + 40..offset + 44].try_into().unwrap()) != 0;
            let max_stack = u32::from_le_bytes(data[offset + 48..offset + 52].try_into().unwrap());
            offset += 52;

            // V3: 对齐
            if code_len % 4 != 0 {
                return Err(VerifyError::V3UnalignedCode {
                    func: name,
                    len: code_len,
                });
            }

            if offset + code_len > data.len() {
                return Err(VerifyError::UnexpectedEof("读取指令流"));
            }
            let code_bytes = &data[offset..offset + code_len];
            offset += code_len;

            let mut code = Vec::with_capacity(code_len / 4);
            for (i, chunk) in code_bytes.chunks_exact(4).enumerate() {
                let pc = i * 4;
                let opcode_byte = chunk[0];
                let operand =
                    ((chunk[1] as u32) << 16) | ((chunk[2] as u32) << 8) | (chunk[3] as u32);
                let op = Op::from_opcode(opcode_byte).ok_or(VerifyError::V3InvalidOpcode {
                    func: name.clone(),
                    pc,
                    opcode: opcode_byte,
                })?;
                code.push(Instr::new(op, operand));
            }

            let source_file = read_string(&mut offset)?;

            // 常量池
            let const_count = read_u32(&mut offset)? as usize;
            let mut constants = Vec::with_capacity(const_count);
            for _ in 0..const_count {
                if offset >= data.len() {
                    return Err(VerifyError::UnexpectedEof("读取常量标签"));
                }
                let tag = data[offset];
                offset += 1;
                let c = match tag {
                    1 => {
                        if offset + 8 > data.len() {
                            return Err(VerifyError::UnexpectedEof("常量 float64"));
                        }
                        let f = f64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                        offset += 8;
                        Constant::Number(f)
                    }
                    2 => {
                        let len = read_uvarint(&mut offset)? as usize;
                        if offset + len > data.len() {
                            return Err(VerifyError::UnexpectedEof("常量 String"));
                        }
                        let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
                        offset += len;
                        Constant::String(s)
                    }
                    3 => {
                        let len = read_uvarint(&mut offset)? as usize;
                        if offset + len > data.len() {
                            return Err(VerifyError::UnexpectedEof("常量 BigInt"));
                        }
                        let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
                        offset += len;
                        Constant::BigInt(s)
                    }
                    4 => {
                        if offset >= data.len() {
                            return Err(VerifyError::UnexpectedEof("常量 Bool"));
                        }
                        let b = data[offset] != 0;
                        offset += 1;
                        Constant::Bool(b)
                    }
                    5 => Constant::Null,
                    _ => return Err(VerifyError::UnexpectedEof("未知常量 tag")),
                };
                constants.push(c);
            }

            // Upvalues
            let uv_count = read_u32(&mut offset)? as usize;
            let mut upvalues = Vec::with_capacity(uv_count);
            for _ in 0..uv_count {
                let is_loc = read_u32(&mut offset)? != 0;
                let idx = read_u32(&mut offset)?;
                upvalues.push(UpvalueCapture {
                    index: idx,
                    is_local: is_loc,
                });
            }

            // NativeCallback
            let has_native = read_u32(&mut offset)?;
            if has_native == 1 {
                let _param_count = read_u32(&mut offset)?;
                let instr_count = read_u32(&mut offset)? as usize;
                offset += instr_count * 8;
            }

            // TryTable
            let try_count = read_u32(&mut offset)? as usize;
            let mut try_table = Vec::with_capacity(try_count);
            for _ in 0..try_count {
                let start_pc = read_u32(&mut offset)?;
                let catch_pc = read_u32(&mut offset)?;
                let finally_pc = read_u32(&mut offset)?;
                let has_catch = read_u32(&mut offset)? != 0;
                let has_finally = read_u32(&mut offset)? != 0;
                let end_pc = read_u32(&mut offset)?;
                let catch_end_pc = read_u32(&mut offset)?;
                let finally_end_pc = read_u32(&mut offset)?;
                try_table.push(TryEntry {
                    start_pc,
                    catch_pc,
                    finally_pc,
                    has_catch,
                    has_finally,
                    end_pc,
                    catch_end_pc,
                    finally_end_pc,
                });
            }

            // LineStarts
            let line_count = read_u32(&mut offset)? as usize;
            offset += line_count * 8;

            functions.push(FuncTemplate {
                name,
                num_params,
                num_locals,
                is_var_args,
                is_generator,
                is_async,
                is_arrow,
                code,
                max_stack,
                source_file,
                constants,
                upvalues,
                try_table,
                line_table: Vec::new(),
            });
            header_extras.push(crate::extras::FuncHeaderExtras {
                arguments_slot,
                no_arguments_object,
                new_target_slot,
                inlinable,
            });
        }

        // Classes
        let mut classes = Vec::with_capacity(class_count);
        for _ in 0..class_count {
            let name = read_string(&mut offset)?;
            if offset + 16 > data.len() {
                return Err(VerifyError::UnexpectedEof("类标量头"));
            }
            let has_super = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) != 0;
            let constructor_index =
                u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap());
            let method_count =
                u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
            let computed_count =
                u32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap()) as usize;
            offset += 16;

            let mut methods = Vec::with_capacity(method_count);
            for _ in 0..method_count {
                if offset + 12 > data.len() {
                    return Err(VerifyError::UnexpectedEof("类方法标量"));
                }
                let func_index = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                let is_static =
                    u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) != 0;
                let kind = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                offset += 12;

                let m_name = read_string(&mut offset)?;
                methods.push(ClassMethod {
                    name: m_name,
                    func_index,
                    is_static,
                    kind,
                });
            }

            let mut computed_indices = Vec::with_capacity(computed_count);
            for _ in 0..computed_count {
                let ci = read_u32(&mut offset)?;
                computed_indices.push(ci);
            }

            classes.push(ClassTemplate {
                name,
                has_super,
                constructor_index,
                methods,
                computed_indices,
            });
        }

        // 归一化：全部为「无 arguments / 无 new.target」默认标量时还原为空——
        // 保证「手工构造空 header_extras」与「序列化回读」严格等价。
        // 注意不能用 Default（arguments_slot 默认 0），序列化默认值是 -1。
        let header_extras = if header_extras.iter().all(|e| {
            e.arguments_slot == -1
                && e.no_arguments_object
                && e.new_target_slot == -1
                && !e.inlinable
        }) {
            Vec::new()
        } else {
            header_extras
        };
        Ok(Self {
            version,
            functions,
            classes,
            header_extras,
        })
    }

    /// 执行 V1..V16 规范全部验证规则。
    pub fn verify(&self) -> Result<(), VerifyError> {
        let total_templates = self.functions.len() + self.classes.len();

        for func in &self.functions {
            let code_len = func.code.len() * 4;

            // 1. 操作数静态边界与类型校验（V4, V5, V6, V7, V15, V16）
            for (idx, instr) in func.code.iter().enumerate() {
                let pc = idx * 4;

                // V4: 局部槽位越界
                match instr.op.operand_kind() {
                    OperandKind::Slot if instr.operand >= func.num_locals => {
                        return Err(VerifyError::V4SlotOutOfRange {
                            func: func.name.clone(),
                            pc,
                            slot: instr.operand,
                            max: func.num_locals,
                        });
                    }
                    OperandKind::PackedSlotName => {
                        let slot = instr.operand >> 16;
                        if slot >= func.num_locals {
                            return Err(VerifyError::V4SlotOutOfRange {
                                func: func.name.clone(),
                                pc,
                                slot,
                                max: func.num_locals,
                            });
                        }
                    }
                    _ => {}
                }

                // V5: 常量池索引越界
                let const_idx = match instr.op.operand_kind() {
                    OperandKind::ConstIdx => Some(instr.operand as usize),
                    OperandKind::PackedSlotName | OperandKind::PackedCall => {
                        Some((instr.operand & 0xFFFF) as usize)
                    }
                    _ => None,
                };

                if let Some(c_idx) = const_idx {
                    if c_idx >= func.constants.len() {
                        return Err(VerifyError::V5ConstOutOfRange {
                            func: func.name.clone(),
                            pc,
                            index: c_idx as u32,
                            max: func.constants.len(),
                        });
                    }

                    // V6: 常量池类型匹配
                    // 对于属性访问、全局访问以及方法调用，常量必须是 String
                    let requires_string = matches!(
                        instr.op,
                        Op::LoadGlobal
                            | Op::StoreGlobal
                            | Op::TypeofGlobal
                            | Op::GetProp
                            | Op::SetProp
                            | Op::SetPropObj
                            | Op::SetPropTop
                            | Op::DelProp
                            | Op::SetGetterObj
                            | Op::SetSetterObj
                            | Op::GetPropLocal
                            | Op::CallMethod
                            | Op::CallMethodArgs
                    );

                    if requires_string {
                        let c = &func.constants[c_idx];
                        if !matches!(c, Constant::String(_)) {
                            return Err(VerifyError::V6ConstTypeMismatch {
                                func: func.name.clone(),
                                pc,
                                expected: "String",
                                actual: c.kind_name(),
                            });
                        }
                    }
                }

                // V7: 跳转目标合法性
                if instr.op.is_jump() {
                    let signed_off = if instr.operand & 0x80_0000 != 0 {
                        (instr.operand | 0xFF00_0000) as i32
                    } else {
                        instr.operand as i32
                    };
                    let target_pc = (pc as i32) + 4 + signed_off;
                    if target_pc < 0 || target_pc > (code_len as i32) || target_pc % 4 != 0 {
                        return Err(VerifyError::V7BadJumpTarget {
                            func: func.name.clone(),
                            pc,
                            target_pc,
                        });
                    }
                }

                // V15: 模板与 Try 表索引范围
                match instr.op.operand_kind() {
                    OperandKind::TemplateIdx if (instr.operand as usize) >= total_templates => {
                        return Err(VerifyError::V15TemplateOutOfRange {
                            func: func.name.clone(),
                            pc,
                            index: instr.operand,
                            max: total_templates,
                        });
                    }
                    OperandKind::TryIdx if (instr.operand as usize) >= func.try_table.len() => {
                        return Err(VerifyError::V15TryOutOfRange {
                            func: func.name.clone(),
                            pc,
                            index: instr.operand,
                            max: func.try_table.len(),
                        });
                    }
                    _ => {}
                }

                // V16: 上值捕获索引范围
                if instr.op.operand_kind() == OperandKind::UpvalueIdx
                    && (instr.operand as usize) >= func.upvalues.len()
                {
                    return Err(VerifyError::V16UpvalueOutOfRange {
                        func: func.name.clone(),
                        pc,
                        index: instr.operand,
                        max: func.upvalues.len(),
                    });
                }
            }

            // 2. TryTable 结构完整性校验（V11, V12, V13, V14）
            for (i, entry) in func.try_table.iter().enumerate() {
                // V11: StartPC < EndPC 且合法对齐
                if entry.start_pc >= entry.end_pc
                    || entry.start_pc % 4 != 0
                    || entry.end_pc % 4 != 0
                    || entry.end_pc > (code_len as u32)
                {
                    return Err(VerifyError::V11BadTryRange {
                        func: func.name.clone(),
                        start_pc: entry.start_pc,
                        end_pc: entry.end_pc,
                    });
                }

                // V12: Handler 在 Body 之外
                if entry.catch_pc != 0
                    && entry.catch_pc >= entry.start_pc
                    && entry.catch_pc < entry.end_pc
                {
                    return Err(VerifyError::V12HandlerInsideBody {
                        func: func.name.clone(),
                        handler_pc: entry.catch_pc,
                        start_pc: entry.start_pc,
                        end_pc: entry.end_pc,
                    });
                }
                if entry.finally_pc != 0
                    && entry.finally_pc >= entry.start_pc
                    && entry.finally_pc < entry.end_pc
                {
                    return Err(VerifyError::V12HandlerInsideBody {
                        func: func.name.clone(),
                        handler_pc: entry.finally_pc,
                        start_pc: entry.start_pc,
                        end_pc: entry.end_pc,
                    });
                }

                // V14: 结束边界有效性
                if entry.catch_end_pc % 4 != 0 || entry.catch_end_pc > (code_len as u32) {
                    return Err(VerifyError::V14BadTryEnd {
                        func: func.name.clone(),
                        field: "catch_end_pc",
                        pc: entry.catch_end_pc,
                    });
                }
                if entry.finally_end_pc % 4 != 0 || entry.finally_end_pc > (code_len as u32) {
                    return Err(VerifyError::V14BadTryEnd {
                        func: func.name.clone(),
                        field: "finally_end_pc",
                        pc: entry.finally_end_pc,
                    });
                }

                // V13: Try 区间嵌套合法性（禁止交叉重叠）
                for other in &func.try_table[i + 1..] {
                    let has_overlap =
                        entry.start_pc < other.end_pc && other.start_pc < entry.end_pc;
                    if has_overlap {
                        let a_in_b =
                            entry.start_pc >= other.start_pc && entry.end_pc <= other.end_pc;
                        let b_in_a =
                            other.start_pc >= entry.start_pc && other.end_pc <= entry.end_pc;
                        if !a_in_b && !b_in_a {
                            return Err(VerifyError::V13TryCrossOverlap {
                                func: func.name.clone(),
                                range_a: (entry.start_pc, entry.end_pc),
                                range_b: (other.start_pc, other.end_pc),
                            });
                        }
                    }
                }
            }

            // 3. 数据流分析：栈深度传播、跨块合流与下溢校验（V8, V9, V10）
            let num_instrs = func.code.len();
            if num_instrs == 0 {
                continue;
            }

            let mut entry_depth: Vec<Option<i32>> = vec![None; num_instrs + 1];
            let mut worklist: VecDeque<usize> = VecDeque::new();

            entry_depth[0] = Some(0);
            worklist.push_back(0);

            // 异常处理器 Catch/Finally 作为入口点加入分析（栈深初始为 1，即捕获的异常对象）
            for entry in &func.try_table {
                if entry.catch_pc != 0 {
                    let idx = (entry.catch_pc / 4) as usize;
                    if idx <= num_instrs && entry_depth[idx].is_none() {
                        entry_depth[idx] = Some(1);
                        worklist.push_back(idx);
                    }
                }
                if entry.finally_pc != 0 {
                    let idx = (entry.finally_pc / 4) as usize;
                    if idx <= num_instrs && entry_depth[idx].is_none() {
                        entry_depth[idx] = Some(0);
                        worklist.push_back(idx);
                    }
                }
            }

            while let Some(idx) = worklist.pop_front() {
                if idx >= num_instrs {
                    continue;
                }
                let pc = idx * 4;
                let current_depth = entry_depth[idx].unwrap();
                let instr = func.code[idx];

                // 精确计算指令的操作数弹出数与净栈变动（兼顾动态变长操作数）
                let (required_pops, net_effect) = match instr.op {
                    Op::NewObject => {
                        let count = instr.operand;
                        ((count * 2) as u8, 1 - (count * 2) as i32)
                    }
                    Op::NewArray => {
                        let count = instr.operand;
                        (count as u8, 1 - count as i32)
                    }
                    Op::Call | Op::New | Op::CallThis | Op::ConstructThis => {
                        let count = instr.operand;
                        ((count + 1) as u8, -(count as i32))
                    }
                    Op::CallMethod => {
                        let num_args = instr.operand >> 16;
                        ((num_args + 1) as u8, -(num_args as i32))
                    }
                    Op::CallWithThis => {
                        let count = instr.operand;
                        ((count + 2) as u8, -(count as i32) - 1)
                    }
                    Op::MakeClass => {
                        let cls_idx = instr.operand as usize;
                        if cls_idx < self.classes.len() {
                            let cls = &self.classes[cls_idx];
                            let pops = (if cls.has_super { 1 } else { 0 })
                                + cls.computed_indices.len() as u8;
                            (pops, 1 - (pops as i32))
                        } else {
                            (0, 1)
                        }
                    }
                    Op::CallWithThisArgs => (3, -2),
                    Op::CallArgs
                    | Op::CallMethodArgs
                    | Op::NewArgs
                    | Op::CallThisArgs
                    | Op::ConstructThisArgs => (2, -1),
                    _ => (instr.op.pops(), instr.op.stack_effect()),
                };

                // V9: 检查栈下溢
                if current_depth < (required_pops as i32) {
                    return Err(VerifyError::V9StackUnderflow {
                        func: func.name.clone(),
                        pc,
                        depth: current_depth,
                        required: required_pops,
                    });
                }

                // 计算后继栈深度与分支后继
                let mut successors: Vec<(usize, i32)> = Vec::new();

                if instr.op.is_terminal() {
                    // RETURN / THROW 等终结指令，无顺序后继
                } else if instr.op == Op::Jmp || instr.op == Op::TryExitJmp {
                    // 无条件跳转
                    let signed_off = if instr.operand & 0x80_0000 != 0 {
                        (instr.operand | 0xFF00_0000) as i32
                    } else {
                        instr.operand as i32
                    };
                    let target_idx = (((pc as i32) + 4 + signed_off) / 4) as usize;
                    let next_depth = current_depth + net_effect;
                    successors.push((target_idx, next_depth));
                } else if matches!(instr.op, Op::Yield | Op::Await) {
                    // 协程挂起与恢复：恢复时 VM 压入恢复参数或 Await 结果，后继栈深保持不变
                    successors.push((idx + 1, current_depth));
                } else if instr.op == Op::OptionalJump {
                    // OptionalJump 短路跳转与 fall through 栈深均不变
                    let signed_off = if instr.operand & 0x80_0000 != 0 {
                        (instr.operand | 0xFF00_0000) as i32
                    } else {
                        instr.operand as i32
                    };
                    let target_idx = (((pc as i32) + 4 + signed_off) / 4) as usize;
                    successors.push((target_idx, current_depth));
                    successors.push((idx + 1, current_depth));
                } else if instr.op.is_jump() {
                    // 条件跳转
                    let signed_off = if instr.operand & 0x80_0000 != 0 {
                        (instr.operand | 0xFF00_0000) as i32
                    } else {
                        instr.operand as i32
                    };
                    let target_idx = (((pc as i32) + 4 + signed_off) / 4) as usize;

                    if matches!(
                        instr.op,
                        Op::JmpTrueKeep | Op::JmpFalseKeep | Op::JmpNullishKeep
                    ) {
                        // Keep 指令：跳转时保留操作数，不跳转时弹栈
                        successors.push((target_idx, current_depth));
                        successors.push((idx + 1, current_depth - 1));
                    } else {
                        // Pop 指令：无论是否跳转均弹栈
                        let next_depth = current_depth + net_effect;
                        successors.push((target_idx, next_depth));
                        successors.push((idx + 1, next_depth));
                    }
                } else {
                    // 普通顺序指令
                    let next_depth = current_depth + net_effect;
                    successors.push((idx + 1, next_depth));
                }

                for (succ_idx, succ_depth) in successors {
                    // V10: 最大栈深超出
                    if succ_depth > (func.max_stack as i32) {
                        return Err(VerifyError::V10MaxStackExceeded {
                            func: func.name.clone(),
                            pc,
                            depth: succ_depth,
                            max_stack: func.max_stack,
                        });
                    }

                    if let Some(existing) = entry_depth[succ_idx] {
                        // V8: 汇合点栈深度必须一致
                        if existing != succ_depth {
                            return Err(VerifyError::V8StackDepthMismatch {
                                func: func.name.clone(),
                                pc: succ_idx * 4,
                                expected: existing,
                                actual: succ_depth,
                            });
                        }
                    } else {
                        entry_depth[succ_idx] = Some(succ_depth);
                        worklist.push_back(succ_idx);
                    }
                }
            }
        }

        Ok(())
    }
}
