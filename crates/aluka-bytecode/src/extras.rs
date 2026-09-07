//! 函数模板标量头的扩展字段旁路读取（v12+ 序列化布局）。
//!
//! `FuncTemplate` 的冻结面不含 `ArgumentsSlot` / `NoArgumentsObject` 等
//! v12+ 扩展标量（见 `AGENTS.md` 冻结纪律：不因旁路数据变更共享结构体）；
//! 本模块以**只读偏移遍历**的方式从字节码中提取这些字段，供 CJS `arguments`
//! 注入等运行时特性使用。偏移推进逻辑与 `deserialize` 严格一致。

use crate::verifier::VerifyError;

/// 函数模板标量头的扩展字段。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FuncHeaderExtras {
    /// `arguments` 对象的局部槽位（负数表示无）
    pub arguments_slot: i32,
    /// 函数体未引用 `arguments`（运行时可跳过注入，O-5 优化标记）
    pub no_arguments_object: bool,
    /// `new.target` 词法槽位
    pub new_target_slot: i32,
    /// 小函数内联标记
    pub inlinable: bool,
}

struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, VerifyError> {
        if self.offset + 4 > self.data.len() {
            return Err(VerifyError::UnexpectedEof("读取 u32"));
        }
        let v = u32::from_le_bytes(self.data[self.offset..self.offset + 4].try_into().unwrap());
        self.offset += 4;
        Ok(v)
    }

    fn byte(&mut self) -> Result<u8, VerifyError> {
        if self.offset >= self.data.len() {
            return Err(VerifyError::UnexpectedEof("读取字节"));
        }
        let v = self.data[self.offset];
        self.offset += 1;
        Ok(v)
    }

    fn uvarint(&mut self) -> Result<u64, VerifyError> {
        let mut result: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            result |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift > 63 {
                return Err(VerifyError::UnexpectedEof("uvarint 溢出"));
            }
        }
    }

    fn string(&mut self) -> Result<String, VerifyError> {
        // 字符串长度前缀是定长 u32（uvarint 仅用于常量池 String/BigInt payload）
        let len = self.u32()? as usize;
        if self.offset + len > self.data.len() {
            return Err(VerifyError::UnexpectedEof("读取字符串"));
        }
        let s = String::from_utf8_lossy(&self.data[self.offset..self.offset + len]).to_string();
        self.offset += len;
        Ok(s)
    }

    fn skip(&mut self, n: usize) -> Result<(), VerifyError> {
        if self.offset + n > self.data.len() {
            return Err(VerifyError::UnexpectedEof("跳过字节"));
        }
        self.offset += n;
        Ok(())
    }

    /// 跳过一个常量（tag + payload），与 `deserialize` 的读取严格一致。
    fn skip_constant(&mut self) -> Result<(), VerifyError> {
        let tag = self.byte()?;
        match tag {
            1 => self.skip(8),
            2 | 3 => {
                let len = self.uvarint()? as usize;
                self.skip(len)
            }
            4 => self.skip(1),
            5 => Ok(()),
            _ => Err(VerifyError::UnexpectedEof("未知常量 tag")),
        }
    }

    /// 跳过一个完整的函数模板（name + 52 标量头 + code + source + constants
    /// + upvalues + native + try_table + line_starts），并读取其扩展标量头。
    fn skip_func_and_read_extras(&mut self) -> Result<FuncHeaderExtras, VerifyError> {
        let _name = self.string()?;
        if self.offset + 52 > self.data.len() {
            return Err(VerifyError::UnexpectedEof("函数标量头"));
        }
        let code_len = u32::from_le_bytes(
            self.data[self.offset + 24..self.offset + 28]
                .try_into()
                .unwrap(),
        ) as usize;
        let arguments_slot = i32::from_le_bytes(
            self.data[self.offset + 28..self.offset + 32]
                .try_into()
                .unwrap(),
        );
        let no_arguments_object = u32::from_le_bytes(
            self.data[self.offset + 32..self.offset + 36]
                .try_into()
                .unwrap(),
        ) != 0;
        let new_target_slot = i32::from_le_bytes(
            self.data[self.offset + 36..self.offset + 40]
                .try_into()
                .unwrap(),
        );
        let inlinable = u32::from_le_bytes(
            self.data[self.offset + 40..self.offset + 44]
                .try_into()
                .unwrap(),
        ) != 0;
        self.skip(52)?;

        // code
        self.skip(code_len)?;
        // source_file
        let _source = self.string()?;
        // constants
        let const_count = self.u32()? as usize;
        for _ in 0..const_count {
            self.skip_constant()?;
        }
        // upvalues（每个 2×u32）
        let uv_count = self.u32()? as usize;
        self.skip(uv_count * 8)?;
        // native callback
        let has_native = self.u32()?;
        if has_native == 1 {
            let _param_count = self.u32()?;
            let instr_count = self.u32()? as usize;
            self.skip(instr_count * 8)?;
        }
        // try_table（每项 8×u32）
        let try_count = self.u32()? as usize;
        self.skip(try_count * 32)?;
        // line_starts（每项 8 字节）
        let line_count = self.u32()? as usize;
        self.skip(line_count * 8)?;

        Ok(FuncHeaderExtras {
            arguments_slot,
            no_arguments_object,
            new_target_slot,
            inlinable,
        })
    }
}

/// 从字节码中读取全部函数模板的扩展标量头（顺序与函数表一致）。
///
/// # Errors
/// 字节码截断/格式不符时返回 [`VerifyError`]。
pub fn read_all_func_header_extras(
    data: &[u8],
    func_count: usize,
) -> Result<Vec<FuncHeaderExtras>, VerifyError> {
    let mut cur = Cursor { data, offset: 0 };
    // 模块头：magic(8) + version(4) + func_count(4) + class_count(4)
    cur.skip(20)?;
    let mut out = Vec::with_capacity(func_count);
    for _ in 0..func_count {
        out.push(cur.skip_func_and_read_extras()?);
    }
    Ok(out)
}
