//! `zlib` 内置模块（Phase 4）：gzip/deflate/brotli/zstd 压缩解压与 CRC-32。
//!
//! 语义实测对齐 Node.js 22 LTS 规范：
//! - 同步族 `*Sync`：JS 线程内直接执行压缩，返回 Buffer 实例；
//! - 异步族（同名非 Sync，如 `gzip(buf, cb)`）：结果经宏任务投递给回调
//!   `(null, Buffer)`，出错时回调 `(errMessage,)`（对齐 Go 的 makeZlibAsync）；
//! - `unzipSync` 按魔数 `1f 8b` 自动识别 gzip，否则按 zlib inflate；
//! - `crc32(data[, value])`：手写 IEEE CRC-32 查表法，确定性输出；
//! - `constants`：flush 模式、返回码、压缩级别/策略与 Brotli、zstd 参数枚举。
//!
//! 对拍纪律：压缩字节流 Go（compress/*）与 Rust（flate2/brotli/自写 zstd 帧）
//! 不逐字节相同，探针一律使用 roundtrip 结果与确定性值（解压文本、crc32 数值、
//! 常量值），绝不打印压缩后的原始字节。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::sync::OnceLock;

use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("zlib")` / `require("node:zlib")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "zlib",
    build,
};

/// 压缩变换统一签名：字节进，字节出，错误带消息（对拍探针只依赖 roundtrip）。
type Transform = fn(&[u8]) -> Result<Vec<u8>, String>;

// --- 压缩实现（gzip / deflate / raw deflate） -------------------------------

fn gzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

fn gunzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn deflate_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

fn inflate_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn deflate_raw_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

fn inflate_raw_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = flate2::read::DeflateDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// 自动识别 gzip（魔数 `1f 8b`）与 zlib 流（对齐 Go 的 unzipBytes）。
fn unzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        gunzip_bytes(data)
    } else {
        inflate_bytes(data)
    }
}

// --- 压缩实现（brotli） ------------------------------------------------------

fn brotli_compress_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        // 质量与窗口取 brotli 默认（11 / 22），写入结束后 Drop 完成 FINISH 块。
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn brotli_decompress_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = brotli::Decompressor::new(data, 4096);
    let mut out = Vec::new();
    reader.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

// --- 压缩实现（zstd：自写 Raw 块帧 + ruzstd 解码） ---------------------------

/// Raw 块上限（zstd 规范单块 128KB）。
const ZSTD_BLOCK_MAX: usize = 1 << 17;

/// 最小合法 zstd 帧写入器：Magic（`28 B5 2F FD` 小端）+ Frame_Header
/// （Single_Segment + 8 字节 Frame_Content_Size）+ Raw 块序列（大输入按
/// 128KB 分块，末块置 Last_Block）。ruzstd 与 Go zstd 均可解。
fn zstd_compress_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len() + 24);
    out.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]);
    // Frame_Header_Descriptor：FCS_field_size = 8（flag 3）| Single_Segment = 1，
    // 无内容校验和、无字典 ID。
    out.push(0xE0);
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    if data.is_empty() {
        // 空帧也须至少一个块：空 Raw 块并置 Last_Block。
        out.extend_from_slice(&[0x01, 0x00, 0x00]);
        return Ok(out);
    }
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + ZSTD_BLOCK_MAX).min(data.len());
        let size = end - offset;
        let last = end == data.len();
        // Block_Header（3 字节小端）：bit0 = Last_Block，bit1..2 = Raw_Block(0)，
        // 其余为块大小。
        let header = ((size as u32) << 3) | u32::from(last);
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(&data[offset..end]);
        offset = end;
    }
    Ok(out)
}

fn zstd_decompress_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder =
        ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| format!("zstd: {e}"))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd: {e}"))?;
    Ok(out)
}

// --- CRC-32（IEEE，手写查表法） ----------------------------------------------

/// IEEE CRC-32 反射多项式 0xEDB88320 的查表（惰性初始化）。
fn crc32_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    })
}

/// CRC-32 增量更新（对齐 Go `crc32.Update` 与 Node `zlib.crc32(data[, value])`：
/// 入口/出口各取一次反码，初值缺省 0）。
fn crc32_update(init: u32, data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc = !init;
    for &byte in data {
        let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = table[idx] ^ (crc >> 8);
    }
    !crc
}

// --- 异步投递（对齐 Go makeZlibAsync 的 (err, result) 回调约定） -------------

/// 一次待投递的异步压缩结果：用户回调 + 成功字节或错误消息。
struct AsyncDelivery {
    callback: Value,
    outcome: Result<Vec<u8>, String>,
}

// 待投递队列（与宏任务 FIFO 对齐：每次异步调用排队一项 + 一个投递宏任务）。
thread_local! {
    // DELIVERIES：线程局部（堆句柄仅本线程 Vm 有效）。
    static DELIVERIES: RefCell<Option<VecDeque<AsyncDelivery>>> = const { RefCell::new(None) };
}

fn queue_delivery(callback: Value, outcome: Result<Vec<u8>, String>) {
    DELIVERIES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(VecDeque::new)
            .push_back(AsyncDelivery { callback, outcome });
    });
}

/// 投递宏任务处理器：从队列取出结果，回调 `(null, Buffer)` 或 `(errMessage,)`。
fn deliver_async(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let delivery = DELIVERIES.with(|g| g.borrow_mut().as_mut().and_then(VecDeque::pop_front));
    let Some(delivery) = delivery else {
        return Ok(Value::Undefined);
    };
    match delivery.outcome {
        Ok(bytes) => {
            let buf = Value::Object(create_buffer_instance(vm, bytes));
            vm.invoke_callable(delivery.callback, Value::Undefined, &[Value::Null, buf])?;
        }
        Err(message) => {
            let msg = Value::Object(vm.alloc_string(message));
            vm.invoke_callable(delivery.callback, Value::Undefined, &[msg])?;
        }
    }
    Ok(Value::Undefined)
}

// --- 通用辅助 ---------------------------------------------------------------

/// 抛出 JS 异常（字符串消息）。
fn thrown(vm: &mut Vm, message: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_string(message.to_owned())))
}

/// 取参数字节：Buffer/数组走 `extract_bytes`，其余按字符串字节（对齐 Go
/// 的 zlibBufferArg：AsBuffer 失败即 `String()`）。
fn buffer_arg(vm: &mut Vm, val: Option<Value>) -> Result<Vec<u8>, VmError> {
    let Some(val) = val else {
        return Err(thrown(vm, "zlib: missing buffer argument"));
    };
    if let Some(bytes) = extract_bytes(vm, val) {
        return Ok(bytes);
    }
    Ok(vm.format_value(val).into_bytes())
}

/// 判断值是否为可调用函数（JS 闭包或原生函数）。
fn is_function(vm: &Vm, val: Value) -> bool {
    matches!(val.case(), ValueCase::Object(r) if matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. })
        )
    )
}

/// 识别回调：`(buf, cb)` 或 `(buf, options, cb)`（对齐 Go makeZlibAsync）。
fn pick_callback(vm: &Vm, args: &[Value]) -> Value {
    if let Some(&second) = args.get(1) {
        if is_function(vm, second) {
            return second;
        }
        if matches!(second.case(), ValueCase::Object(_)) {
            if let Some(&third) = args.get(2) {
                if is_function(vm, third) {
                    return third;
                }
            }
        }
    }
    Value::Undefined
}

/// 同步执行包装：压缩后包成 Buffer 返回（错误前缀 `zlib: `，对齐 makeZlibSync）。
fn run_sync(vm: &mut Vm, args: &[Value], func: Transform) -> Result<Value, VmError> {
    let data = buffer_arg(vm, args.first().copied())?;
    let out = func(&data).map_err(|e| thrown(vm, &format!("zlib: {e}")))?;
    Ok(Value::Object(create_buffer_instance(vm, out)))
}

/// 异步执行包装：排队投递宏任务，回调 `(null, Buffer)` / `(errMessage,)`。
fn run_async(vm: &mut Vm, args: &[Value], func: Transform) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(thrown(vm, "zlib: missing buffer argument"));
    }
    let data = buffer_arg(vm, args.first().copied())?;
    let callback = pick_callback(vm, args);
    queue_delivery(callback, func(&data));
    // 排投递宏任务（线性驱动模型：宏任务排空时执行投递处理器）。
    let deliver_fn = vm.alloc_native_fn("zlib.asyncDeliver");
    vm.timer_counter += 1;
    let id = vm.timer_counter;
    let last_due = vm.macro_tasks.back().map(|(_, d, _, _, _)| *d).unwrap_or(0);
    vm.macro_tasks
        .push_back((id, last_due, 0, Value::Object(deliver_fn), false));
    Ok(Value::Undefined)
}

// --- 同步 API 处理器（薄包装，逐个绑定变换函数） ------------------------------

macro_rules! sync_handler {
    ($name:ident, $func:expr, $doc:literal) => {
        #[doc = $doc]
        fn $name(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
            run_sync(vm, args, $func)
        }
    };
}

sync_handler!(
    gzip_sync,
    gzip_bytes,
    "`zlib.gzipSync(buf)`：同步 gzip 压缩。"
);
sync_handler!(
    gunzip_sync,
    gunzip_bytes,
    "`zlib.gunzipSync(buf)`：同步 gzip 解压。"
);
sync_handler!(
    deflate_sync,
    deflate_bytes,
    "`zlib.deflateSync(buf)`：同步 zlib 压缩。"
);
sync_handler!(
    inflate_sync,
    inflate_bytes,
    "`zlib.inflateSync(buf)`：同步 zlib 解压。"
);
sync_handler!(
    deflate_raw_sync,
    deflate_raw_bytes,
    "`zlib.deflateRawSync(buf)`：同步 raw deflate 压缩。"
);
sync_handler!(
    inflate_raw_sync,
    inflate_raw_bytes,
    "`zlib.inflateRawSync(buf)`：同步 raw deflate 解压。"
);
sync_handler!(
    unzip_sync,
    unzip_bytes,
    "`zlib.unzipSync(buf)`：自动识别 gzip/zlib 解压。"
);
sync_handler!(
    brotli_compress_sync,
    brotli_compress_bytes,
    "`zlib.brotliCompressSync(buf)`：同步 brotli 压缩。"
);
sync_handler!(
    brotli_decompress_sync,
    brotli_decompress_bytes,
    "`zlib.brotliDecompressSync(buf)`：同步 brotli 解压。"
);
sync_handler!(
    zstd_compress_sync,
    zstd_compress_bytes,
    "`zlib.zstdCompressSync(buf)`：同步 zstd 压缩（最小 Raw 块帧）。"
);
sync_handler!(
    zstd_decompress_sync,
    zstd_decompress_bytes,
    "`zlib.zstdDecompressSync(buf)`：同步 zstd 解压（ruzstd）。"
);

// --- 异步 API 处理器（同名非 Sync） ------------------------------------------

macro_rules! async_handler {
    ($name:ident, $func:expr, $doc:literal) => {
        #[doc = $doc]
        fn $name(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
            run_async(vm, args, $func)
        }
    };
}

async_handler!(
    gzip_async,
    gzip_bytes,
    "`zlib.gzip(buf, cb)`：异步 gzip 压缩。"
);
async_handler!(
    gunzip_async,
    gunzip_bytes,
    "`zlib.gunzip(buf, cb)`：异步 gzip 解压。"
);
async_handler!(
    deflate_async,
    deflate_bytes,
    "`zlib.deflate(buf, cb)`：异步 zlib 压缩。"
);
async_handler!(
    inflate_async,
    inflate_bytes,
    "`zlib.inflate(buf, cb)`：异步 zlib 解压。"
);
async_handler!(
    deflate_raw_async,
    deflate_raw_bytes,
    "`zlib.deflateRaw(buf, cb)`：异步 raw deflate 压缩。"
);
async_handler!(
    inflate_raw_async,
    inflate_raw_bytes,
    "`zlib.inflateRaw(buf, cb)`：异步 raw deflate 解压。"
);
async_handler!(
    unzip_async,
    unzip_bytes,
    "`zlib.unzip(buf, cb)`：异步自动识别解压。"
);
async_handler!(
    brotli_compress_async,
    brotli_compress_bytes,
    "`zlib.brotliCompress(buf, cb)`：异步 brotli 压缩。"
);
async_handler!(
    brotli_decompress_async,
    brotli_decompress_bytes,
    "`zlib.brotliDecompress(buf, cb)`：异步 brotli 解压。"
);
async_handler!(
    zstd_compress_async,
    zstd_compress_bytes,
    "`zlib.zstdCompress(buf, cb)`：异步 zstd 压缩。"
);
async_handler!(
    zstd_decompress_async,
    zstd_decompress_bytes,
    "`zlib.zstdDecompress(buf, cb)`：异步 zstd 解压。"
);

// --- crc32 与 constants ------------------------------------------------------

/// `zlib.crc32(data[, value])`：IEEE CRC-32 校验和，返回无符号 32 位整数
/// （初值 `value` 缺省为 0；对齐 Go `crc32.Update`）。
fn crc32(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(first) = args.first().copied() else {
        return Err(thrown(vm, "zlib: crc32 requires data"));
    };
    let data = buffer_arg(vm, Some(first))?;
    let mut value = 0u32;
    if let Some(n) = args.get(1).and_then(|v| v.as_number()) {
        value = (n as i64) as u32;
    }
    Ok(Value::Number(f64::from(crc32_update(value, &data))))
}

/// 构造 `zlib.constants` 对象（清单逐项照搬 Go 的 zlibConstantsObject）。
fn build_constants(vm: &mut Vm) -> ObjectRef {
    const ITEMS: &[(&str, i64)] = &[
        // flush 模式
        ("Z_NO_FLUSH", 0),
        ("Z_PARTIAL_FLUSH", 1),
        ("Z_SYNC_FLUSH", 2),
        ("Z_FULL_FLUSH", 3),
        ("Z_FINISH", 4),
        ("Z_BLOCK", 5),
        // 返回码
        ("Z_OK", 0),
        ("Z_STREAM_END", 1),
        ("Z_NEED_DICT", 2),
        ("Z_ERRNO", -1),
        ("Z_STREAM_ERROR", -2),
        ("Z_DATA_ERROR", -3),
        ("Z_MEM_ERROR", -4),
        ("Z_BUF_ERROR", -5),
        ("Z_VERSION_ERROR", -6),
        // 压缩级别
        ("Z_NO_COMPRESSION", 0),
        ("Z_BEST_SPEED", 1),
        ("Z_BEST_COMPRESSION", 9),
        ("Z_DEFAULT_COMPRESSION", -1),
        // 压缩策略
        ("Z_FILTERED", 1),
        ("Z_HUFFMAN_ONLY", 2),
        ("Z_RLE", 3),
        ("Z_FIXED", 4),
        ("Z_DEFAULT_STRATEGY", 0),
        // Brotli 模式/操作/参数
        ("BROTLI_MODE_GENERIC", 0),
        ("BROTLI_MODE_TEXT", 1),
        ("BROTLI_MODE_FONT", 2),
        ("BROTLI_OPERATION_PROCESS", 0),
        ("BROTLI_OPERATION_FLUSH", 1),
        ("BROTLI_OPERATION_FINISH", 2),
        ("BROTLI_OPERATION_EMIT_METADATA", 3),
        ("BROTLI_PARAM_MODE", 0),
        ("BROTLI_PARAM_QUALITY", 1),
        ("BROTLI_PARAM_LGWIN", 2),
        ("BROTLI_PARAM_LGBLOCK", 3),
        ("BROTLI_PARAM_DISABLE_LITERAL_CONTEXT_MODELING", 4),
        ("BROTLI_PARAM_SIZE_HINT", 5),
        ("BROTLI_PARAM_LARGE_WINDOW", 6),
        ("BROTLI_PARAM_NPOSTFIX", 7),
        ("BROTLI_PARAM_NDIRECT", 8),
        ("BROTLI_MIN_QUALITY", 0),
        ("BROTLI_MAX_QUALITY", 11),
        ("BROTLI_DEFAULT_QUALITY", 11),
        ("BROTLI_MIN_WINDOW_BITS", 10),
        ("BROTLI_MAX_WINDOW_BITS", 24),
        ("BROTLI_DEFAULT_WINDOW", 22),
        ("BROTLI_MIN_INPUT_BLOCK_BITS", 16),
        ("BROTLI_MAX_INPUT_BLOCK_BITS", 24),
        ("BROTLI_DEFAULT_LG_BLOCK", 22),
        ("BROTLI_DECODER_RESULT_ERROR", 0),
        ("BROTLI_DECODER_RESULT_SUCCESS", 1),
        ("BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT", 2),
        ("BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT", 3),
        // zstd 参数枚举（libzstd ZSTD_cParameter 值，与 Node 一致）
        ("ZSTD_c_compressionLevel", 100),
        ("ZSTD_c_strategy", 107),
    ];
    let obj = vm.alloc_ordinary();
    for (name, value) in ITEMS {
        let _ = vm.set_property(Value::Object(obj), name, Value::Number(*value as f64));
    }
    obj
}

// --- 模块注册 ----------------------------------------------------------------

/// 同步 API 注册表：导出名 → 处理器（处理器内部绑定变换函数）。
const SYNC_TABLE: &[(&str, crate::builtins::BuiltinHandler)] = &[
    ("gzipSync", gzip_sync),
    ("gunzipSync", gunzip_sync),
    ("deflateSync", deflate_sync),
    ("inflateSync", inflate_sync),
    ("deflateRawSync", deflate_raw_sync),
    ("inflateRawSync", inflate_raw_sync),
    ("unzipSync", unzip_sync),
    ("brotliCompressSync", brotli_compress_sync),
    ("brotliDecompressSync", brotli_decompress_sync),
    ("zstdCompressSync", zstd_compress_sync),
    ("zstdDecompressSync", zstd_decompress_sync),
];

/// 异步 API 注册表：导出名 → 处理器（同名非 Sync，回调 `(err, result)`）。
const ASYNC_TABLE: &[(&str, crate::builtins::BuiltinHandler)] = &[
    ("gzip", gzip_async),
    ("gunzip", gunzip_async),
    ("deflate", deflate_async),
    ("inflate", inflate_async),
    ("deflateRaw", deflate_raw_async),
    ("inflateRaw", inflate_raw_async),
    ("unzip", unzip_async),
    ("brotliCompress", brotli_compress_async),
    ("brotliDecompress", brotli_decompress_async),
    ("zstdCompress", zstd_compress_async),
    ("zstdDecompress", zstd_decompress_async),
];

/// 构建 `zlib` 模块单例并注册全部同步/异步处理器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for (name, handler) in SYNC_TABLE {
        let fn_ref = vm.alloc_native_fn(&format!("zlib.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
        register_handler(registry, "zlib", name, *handler);
    }
    for (name, handler) in ASYNC_TABLE {
        let fn_ref = vm.alloc_native_fn(&format!("zlib.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
        register_handler(registry, "zlib", name, *handler);
    }

    let crc_fn = vm.alloc_native_fn("zlib.crc32");
    set_module_prop(vm, obj, "crc32", Value::Object(crc_fn))?;
    register_handler(registry, "zlib", "crc32", crc32);

    // 异步投递处理器：run_async 排队的宏任务以 NativeFn "zlib.asyncDeliver"
    // 触发（invoke_callable 按名称查分派表），不暴露在模块导出面上。
    register_handler(registry, "zlib", "asyncDeliver", deliver_async);

    let constants = build_constants(vm);
    set_module_prop(vm, obj, "constants", Value::Object(constants))?;

    Ok(obj)
}

/// 编译期锚定：确保处理器签名与注册表一致，并单测 zstd 帧自产自销。
/// GC 根快照：待投递异步压缩回调。
pub(crate) fn store_roots(out: &mut crate::gc::GcRoots) {
    DELIVERIES.with(|g| {
        if let Some(queue) = g.borrow().as_ref() {
            for d in queue {
                out.push(d.callback);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_frame_roundtrip_via_ruzstd() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"hello zstd".to_vec(),
            "中文 zstd roundtrip ✓".as_bytes().to_vec(),
            // 跨越 128KB 分块边界
            vec![b'A'; ZSTD_BLOCK_MAX * 2 + 123],
        ];
        for input in cases {
            let frame = zstd_compress_bytes(&input).expect("帧写入成功");
            let mut decoder =
                ruzstd::decoding::StreamingDecoder::new(&frame[..]).expect("帧头合法");
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).expect("解码成功");
            assert_eq!(out, input, "zstd 帧自产自销 roundtrip 必须一致");
        }
    }

    #[test]
    fn gzip_deflate_roundtrip() {
        for func in [gzip_bytes, deflate_bytes, deflate_raw_bytes] {
            let compressed = func(b"payload").expect("压缩成功");
            assert!(!compressed.is_empty());
        }
        assert_eq!(
            gunzip_bytes(&gzip_bytes(b"payload").unwrap()).unwrap(),
            b"payload"
        );
        assert_eq!(
            inflate_bytes(&deflate_bytes(b"payload").unwrap()).unwrap(),
            b"payload"
        );
        assert_eq!(
            inflate_raw_bytes(&deflate_raw_bytes(b"payload").unwrap()).unwrap(),
            b"payload"
        );
        assert_eq!(unzip_bytes(&gzip_bytes(b"x").unwrap()).unwrap(), b"x");
        assert_eq!(unzip_bytes(&deflate_bytes(b"x").unwrap()).unwrap(), b"x");
        assert_eq!(
            brotli_decompress_bytes(&brotli_compress_bytes(b"payload").unwrap()).unwrap(),
            b"payload"
        );
    }

    #[test]
    fn crc32_known_values() {
        // Node/Node.js 22 LTS 标准 实测：crc32("hello") 与 CRC-32 标准校验值
        assert_eq!(crc32_update(0, b"hello"), 907060870);
        assert_eq!(crc32_update(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_update(0, b""), 0);
        assert_eq!(crc32_update(907060870, b"123456789"), 124_049_046);
    }

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = gzip_sync;
        let _: crate::builtins::BuiltinHandler = unzip_sync;
        let _: crate::builtins::BuiltinHandler = zstd_decompress_sync;
        let _: crate::builtins::BuiltinHandler = gzip_async;
        let _: crate::builtins::BuiltinHandler = zstd_decompress_async;
        let _: crate::builtins::BuiltinHandler = crc32;
        let _: crate::builtins::BuiltinHandler = deliver_async;
    }
}
