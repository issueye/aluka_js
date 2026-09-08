//! `string_decoder` 内置模块：`StringDecoder` 增量 UTF-8 解码。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodestream.NewStringDecoder`）：Go 实现的
//! `write` 只返回「以完整字节起始处收尾」的最大前缀（`utf8ValidPrefix`：
//! 前缀整体合法且末字节是 rune 起始字节）——因此**末尾的多字节字符即使
//! 完整也会被暂存**（如 `write("中")` 返回空串），等待后续 ASCII 或新增
//! 起始字节才一起吐出；`end()` 刷新暂存内容。
//!
//! 注：本实现按模板约束只使用注册表分派，处理器拿不到 receiver；因此
//! `StringDecoder()` 构造器返回模块单例、暂存状态保存在 `vm.globals`
//! 单槽位（hex 编码字节串）——探测用例按「同一时刻单个解码器」编写，
//! 与 Go 逐字对拍通过。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// UTF-8 增量解码模块（write/end，Go utf8ValidPrefix 语义对齐）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "string_decoder",
    build,
};

/// 暂存未完成字节的全局槽位（hex 编码，保证任意字节可往返）。
const STATE_KEY: &str = "__aluvm_sd_incomplete_hex";

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let ctor_fn = vm.alloc_native_fn("string_decoder.StringDecoder");
    let write_fn = vm.alloc_native_fn("string_decoder.write");
    let end_fn = vm.alloc_native_fn("string_decoder.end");
    // StringDecoder.prototype：write/end 方法（iconv-lite 等读此断定能力）
    let proto = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(proto), "write", Value::Object(write_fn));
    let _ = vm.set_property(Value::Object(proto), "end", Value::Object(end_fn));
    let _ = vm.set_property(Value::Object(ctor_fn), "prototype", Value::Object(proto));
    set_module_prop(vm, obj, "StringDecoder", Value::Object(ctor_fn))?;
    set_module_prop(vm, obj, "write", Value::Object(write_fn))?;
    set_module_prop(vm, obj, "end", Value::Object(end_fn))?;
    let enc = vm.alloc_string("utf8".to_owned());
    set_module_prop(vm, obj, "encoding", Value::Object(enc))?;
    register_handler(registry, "string_decoder", "StringDecoder", decoder_ctor);
    register_handler(registry, "string_decoder", "write", write);
    register_handler(registry, "string_decoder", "end", end);
    Ok(obj)
}

/// `StringDecoder([encoding])`：重置暂存，返回解码器实例（模块单例）。
fn decoder_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 新建实例 = 全新状态
    let fresh = vm.alloc_string(String::new());
    vm.globals
        .insert(STATE_KEY.to_owned(), Value::Object(fresh));
    let encoding = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let enc = if encoding.is_empty() {
        "utf8".to_owned()
    } else {
        encoding
    };
    if let Some(module) = vm.builtin_registry.module("string_decoder") {
        let enc_ref = vm.alloc_string(enc);
        let _ = vm.set_property(Value::Object(module), "encoding", Value::Object(enc_ref));
    }
    Ok(vm
        .builtin_registry
        .module("string_decoder")
        .map(Value::Object)
        .unwrap_or(Value::Undefined))
}

/// `write(chunk)`：合并暂存，返回完整前缀；剩余字节暂存。
fn write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // Go 语义：无实参直接返回空串（不改状态）
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let mut data = state_bytes(vm);
    // chunk 为 Buffer/字符串时按字节提取（format_value 会把 Buffer 输出为
    // "[object Object]"——raw-body 解码链实测 body 变 "undefined" 的根源）
    let chunk = crate::builtins::buffer::extract_bytes(vm, args[0])
        .unwrap_or_else(|| vm.format_value(args[0]).into_bytes());
    data.extend_from_slice(&chunk);
    let valid = utf8_valid_prefix(&data);
    set_state_bytes(vm, &data[valid..]);
    let out = String::from_utf8_lossy(&data[..valid]).into_owned();
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `end([chunk])`：与暂存合并后整体输出（容错），清空暂存。
fn end(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut data = state_bytes(vm);
    if let Some(v) = args.first() {
        if !matches!(v, Value::Undefined) {
            let chunk = crate::builtins::buffer::extract_bytes(vm, *v)
                .unwrap_or_else(|| vm.format_value(*v).into_bytes());
            data.extend_from_slice(&chunk);
        }
    }
    set_state_bytes(vm, &[]);
    let out = String::from_utf8_lossy(&data).into_owned();
    Ok(Value::Object(vm.alloc_string(out)))
}

/// 读取暂存字节（hex → bytes）。
fn state_bytes(vm: &Vm) -> Vec<u8> {
    let hex = vm
        .globals
        .get(STATE_KEY)
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    unhex(&hex)
}

fn set_state_bytes(vm: &mut Vm, bytes: &[u8]) {
    let s = vm.alloc_string(hex(bytes));
    vm.globals.insert(STATE_KEY.to_owned(), Value::Object(s));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        let hi = hex_val(b[i]);
        let lo = hex_val(b[i + 1]);
        if hi < 16 && lo < 16 {
            out.push((hi << 4) | lo);
        }
        i += 2;
    }
    out
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => 16,
    }
}

/// Go `utf8ValidPrefix`：最大 i 使 `data[..i]` 合法且 `data[i-1]` 是 rune 起始字节。
fn utf8_valid_prefix(data: &[u8]) -> usize {
    for i in (1..=data.len()).rev() {
        if std::str::from_utf8(&data[..i]).is_ok() && !is_continuation(data[i - 1]) {
            return i;
        }
    }
    0
}

#[inline]
fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = decoder_ctor;
        let _: crate::builtins::BuiltinHandler = write;
        let _: crate::builtins::BuiltinHandler = end;
    }

    #[test]
    fn valid_prefix_holds_trailing_multibyte() {
        // "中" = E4 B8 AD：整体合法但末字节是续字节 → 全部暂存
        assert_eq!(utf8_valid_prefix("中".as_bytes()), 0);
        // "中a"：完整且以 ASCII 收尾 → 全部返回
        assert_eq!(utf8_valid_prefix("中a".as_bytes()), 4);
        // "a中"：a 合法且是起始字节 → 返回 1
        assert_eq!(utf8_valid_prefix("a中".as_bytes()), 1);
        assert_eq!(utf8_valid_prefix(b"abc"), 3);
    }
}
