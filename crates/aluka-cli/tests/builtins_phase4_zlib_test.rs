//! Phase 4 `zlib` 与 `stream/web` 内置库端到端与接口对拍测试：
//! - `zlib`：gzip/deflate/deflateRaw/unzip/brotli/zstd 同步与异步族、`crc32`、`constants`；
//! - `stream/web`：`ReadableStream` / `WritableStream` / `TransformStream` 构造器
//!   表面与 `ReadableStreamTee`。
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 对拍纪律：压缩字节流 Go（compress/*）与 Rust（flate2/brotli/自写 zstd 帧）
//! 不逐字节相同，探针一律使用 roundtrip 结果与确定性值（解压文本、crc32 数值、
//! 常量值、异步回调收到的解压文本），绝不打印压缩后的原始字节；异步用例采用
//! 串行链（一次只有一个在途异步操作），规避 Go goroutine 完成顺序的非确定性。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "builtins_phase4_zlib_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 zlib 全部公开导出的 typeof 表面（同步族 / 异步族 / crc32 / constants），
/// 同时覆盖 `require("zlib")`（无 `node:` 前缀）的解析路径。
#[test]
fn zlib_exports_typeof_e2e_matches_go() {
    let work = work_dir("typeof");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"zlib\");\n",
            "console.log(typeof zlib.gzipSync, typeof zlib.gunzipSync, typeof zlib.deflateSync, ",
            "typeof zlib.inflateSync, typeof zlib.deflateRawSync, typeof zlib.inflateRawSync, ",
            "typeof zlib.unzipSync, typeof zlib.brotliCompressSync, typeof zlib.brotliDecompressSync, ",
            "typeof zlib.zstdCompressSync, typeof zlib.zstdDecompressSync, typeof zlib.crc32, ",
            "typeof zlib.constants);\n",
            "console.log(typeof zlib.gzip, typeof zlib.gunzip, typeof zlib.deflate, typeof zlib.inflate, ",
            "typeof zlib.deflateRaw, typeof zlib.inflateRaw, typeof zlib.unzip, typeof zlib.brotliCompress, ",
            "typeof zlib.brotliDecompress, typeof zlib.zstdCompress, typeof zlib.zstdDecompress);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "function function function function function function function function function function \
         function function object\n\
         function function function function function function function function function function function"
    );
}

/// 验证 zlib.constants 常量值与 Go 逐项一致（flush 模式 / 返回码 / 级别 /
/// 策略 / Brotli 枚举 / zstd 参数）。
#[test]
fn zlib_constants_values_e2e_matches_go() {
    let work = work_dir("constants");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const c = require(\"node:zlib\").constants;\n",
            "console.log(c.Z_NO_FLUSH, c.Z_PARTIAL_FLUSH, c.Z_SYNC_FLUSH, c.Z_FULL_FLUSH, c.Z_FINISH, c.Z_BLOCK);\n",
            "console.log(c.Z_OK, c.Z_STREAM_END, c.Z_NEED_DICT, c.Z_ERRNO, c.Z_STREAM_ERROR, ",
            "c.Z_DATA_ERROR, c.Z_MEM_ERROR, c.Z_BUF_ERROR, c.Z_VERSION_ERROR);\n",
            "console.log(c.Z_NO_COMPRESSION, c.Z_BEST_SPEED, c.Z_BEST_COMPRESSION, c.Z_DEFAULT_COMPRESSION);\n",
            "console.log(c.Z_FILTERED, c.Z_HUFFMAN_ONLY, c.Z_RLE, c.Z_FIXED, c.Z_DEFAULT_STRATEGY);\n",
            "console.log(c.BROTLI_MODE_GENERIC, c.BROTLI_MODE_TEXT, c.BROTLI_MODE_FONT, ",
            "c.BROTLI_OPERATION_PROCESS, c.BROTLI_OPERATION_FLUSH, c.BROTLI_OPERATION_FINISH, ",
            "c.BROTLI_OPERATION_EMIT_METADATA);\n",
            "console.log(c.BROTLI_PARAM_MODE, c.BROTLI_PARAM_QUALITY, c.BROTLI_PARAM_LGWIN, ",
            "c.BROTLI_PARAM_LGBLOCK, c.BROTLI_PARAM_DISABLE_LITERAL_CONTEXT_MODELING, ",
            "c.BROTLI_PARAM_SIZE_HINT, c.BROTLI_PARAM_LARGE_WINDOW, c.BROTLI_PARAM_NPOSTFIX, c.BROTLI_PARAM_NDIRECT);\n",
            "console.log(c.BROTLI_MIN_QUALITY, c.BROTLI_MAX_QUALITY, c.BROTLI_DEFAULT_QUALITY, ",
            "c.BROTLI_MIN_WINDOW_BITS, c.BROTLI_MAX_WINDOW_BITS, c.BROTLI_DEFAULT_WINDOW);\n",
            "console.log(c.BROTLI_MIN_INPUT_BLOCK_BITS, c.BROTLI_MAX_INPUT_BLOCK_BITS, c.BROTLI_DEFAULT_LG_BLOCK);\n",
            "console.log(c.BROTLI_DECODER_RESULT_ERROR, c.BROTLI_DECODER_RESULT_SUCCESS, ",
            "c.BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT, c.BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT);\n",
            "console.log(c.ZSTD_c_compressionLevel, c.ZSTD_c_strategy);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "0 1 2 3 4 5\n\
         0 1 2 -1 -2 -3 -4 -5 -6\n\
         0 1 9 -1\n\
         1 2 3 4 0\n\
         0 1 2 0 1 2 3\n\
         0 1 2 3 4 5 6 7 8\n\
         0 11 11 10 24 22\n\
         16 24 22\n\
         0 1 2 3\n\
         100 107"
    );
}

/// 验证 gzip/gunzip roundtrip：解压文本逐字一致（含中文与 Buffer 输入）。
#[test]
fn zlib_gzip_gunzip_roundtrip_e2e_matches_go() {
    let work = work_dir("gzip");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const Buffer = require(\"node:buffer\").Buffer;\n",
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"hello zlib roundtrip 你好\";\n",
            "console.log(zlib.gunzipSync(zlib.gzipSync(s)).toString());\n",
            "console.log(zlib.gunzipSync(zlib.gzipSync(Buffer.from(\"buffer input\"))).toString());\n",
            "console.log(zlib.gunzipSync(zlib.gzipSync(s)).length);\n",
            "console.log(zlib.gunzipSync(zlib.gzipSync(\"\")).toString() === \"\");\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "hello zlib roundtrip 你好\nbuffer input\n27\ntrue");
}

/// 验证 deflate/inflate roundtrip（zlib 包装的 deflate 流）。
#[test]
fn zlib_deflate_inflate_roundtrip_e2e_matches_go() {
    let work = work_dir("deflate");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"deflate me 你好\";\n",
            "console.log(zlib.inflateSync(zlib.deflateSync(s)).toString());\n",
            "console.log(zlib.inflateSync(zlib.deflateSync(\"abc abc abc\")).length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "deflate me 你好\n11");
}

/// 验证 deflateRaw/inflateRaw roundtrip（裸 deflate 流）。
#[test]
fn zlib_deflate_raw_roundtrip_e2e_matches_go() {
    let work = work_dir("deflate_raw");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"raw deflate payload\";\n",
            "console.log(zlib.inflateRawSync(zlib.deflateRawSync(s)).toString());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "raw deflate payload");
}

/// 验证 unzipSync 按魔数自动识别 gzip（`1f 8b`）与 zlib 流（对齐 Go unzipBytes）。
#[test]
fn zlib_unzip_auto_detect_e2e_matches_go() {
    let work = work_dir("unzip");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"unzip both formats\";\n",
            "console.log(zlib.unzipSync(zlib.gzipSync(s)).toString());\n",
            "console.log(zlib.unzipSync(zlib.deflateSync(s)).toString());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "unzip both formats\nunzip both formats");
}

/// 验证 brotli 压缩/解压 roundtrip。
#[test]
fn zlib_brotli_roundtrip_e2e_matches_go() {
    let work = work_dir("brotli");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"brotli roundtrip 数据\";\n",
            "console.log(zlib.brotliDecompressSync(zlib.brotliCompressSync(s)).toString());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "brotli roundtrip 数据");
}

/// 验证 zstd 压缩/解压 roundtrip：小输入 + 跨 128KB 分块边界的大输入
///（只对拍解压长度，不打印压缩字节）。
#[test]
fn zlib_zstd_roundtrip_e2e_matches_go() {
    let work = work_dir("zstd");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var s = \"zstd roundtrip 帧格式\";\n",
            "console.log(zlib.zstdDecompressSync(zlib.zstdCompressSync(s)).toString());\n",
            "var big = \"\";\n",
            "for (var i = 0; i < 20000; i++) { big += \"0123456789\"; }\n",
            "console.log(zlib.zstdDecompressSync(zlib.zstdCompressSync(big)).length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "zstd roundtrip 帧格式\n200000");
}

/// 验证 crc32(data[, value])：IEEE CRC-32 确定性数值（含链式增量更新与空输入）。
#[test]
fn zlib_crc32_values_e2e_matches_go() {
    let work = work_dir("crc32");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const Buffer = require(\"node:buffer\").Buffer;\n",
            "const zlib = require(\"node:zlib\");\n",
            "console.log(zlib.crc32(\"hello\"), zlib.crc32(\"123456789\"), ",
            "zlib.crc32(\"123456789\", zlib.crc32(\"hello\")));\n",
            "console.log(zlib.crc32(\"\"));\n",
            "console.log(zlib.crc32(Buffer.from(\"five!\")), zlib.crc32(\"hello\", 0));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "907060870 3421780262 124049046\n0\n2464241191 907060870"
    );
}

/// 验证异步回调版：串行链 gzip → gunzip → zstdCompress → zstdDecompress，
/// 回调收到 `(null, Buffer)`，最终打印解压文本（对齐 Go makeZlibAsync 语义）。
#[test]
fn zlib_async_serial_chain_e2e_matches_go() {
    let work = work_dir("async_chain");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "zlib.gzip(\"async chain payload\", function(err, buf) {\n",
            "    if (err !== null) { console.log(\"error:\", err); return; }\n",
            "    zlib.gunzip(buf, function(err2, out) {\n",
            "        if (err2 !== null) { console.log(\"error:\", err2); return; }\n",
            "        console.log(\"async chain:\", out.toString());\n",
            "        zlib.zstdCompress(out, function(e3, z) {\n",
            "            if (e3 !== null) { console.log(\"error:\", e3); return; }\n",
            "            zlib.zstdDecompress(z, function(e4, s) {\n",
            "                if (e4 !== null) { console.log(\"error:\", e4); return; }\n",
            "                console.log(\"zstd async:\", s.toString());\n",
            "            });\n",
            "        });\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "async chain: async chain payload\nzstd async: async chain payload"
    );
}

/// 验证异步 deflate/inflate 与 brotli 回调（单在途操作，顺序确定）。
#[test]
fn zlib_async_deflate_inflate_e2e_matches_go() {
    let work = work_dir("async_deflate");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "zlib.deflate(\"d1\", function(e, b) {\n",
            "    console.log(\"deflate async:\", e === null, zlib.inflateSync(b).toString());\n",
            "    zlib.brotliDecompress(zlib.brotliCompressSync(\"bt\"), function(e2, o) {\n",
            "        console.log(\"brotli async:\", o.toString());\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "deflate async: true d1\nbrotli async: bt");
}

/// 验证错误路径：坏输入同步解压必须抛出 JS 异常（只对拍「是否抛出」，
/// 不对拍各实现不同的错误消息文本）。
#[test]
fn zlib_error_paths_throw_e2e_matches_go() {
    let work = work_dir("errors");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var caught = \"\";\n",
            "try { zlib.gunzipSync(\"not gzip\"); } catch (e) { caught += \"a\"; }\n",
            "try { zlib.inflateSync(\"abc\"); } catch (e) { caught += \"b\"; }\n",
            "try { zlib.inflateRawSync(\"abc\"); } catch (e) { caught += \"c\"; }\n",
            "try { zlib.brotliDecompressSync(\"abc\"); } catch (e) { caught += \"d\"; }\n",
            "console.log(\"caught:\", caught);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "caught: abcd");
}

/// 验证大输入（200KB）在 gzip/deflate/deflateRaw/brotli/zstd/unzip 下的
/// roundtrip 长度一致（zstd 跨 128KB 分块边界）。
#[test]
fn zlib_large_input_roundtrip_lengths_e2e_matches_go() {
    let work = work_dir("large");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const zlib = require(\"node:zlib\");\n",
            "var big = \"\";\n",
            "for (var i = 0; i < 20000; i++) { big += \"0123456789\"; }\n",
            "console.log(zlib.gunzipSync(zlib.gzipSync(big)).length);\n",
            "console.log(zlib.inflateSync(zlib.deflateSync(big)).length);\n",
            "console.log(zlib.inflateRawSync(zlib.deflateRawSync(big)).length);\n",
            "console.log(zlib.brotliDecompressSync(zlib.brotliCompressSync(big)).length);\n",
            "console.log(zlib.zstdDecompressSync(zlib.zstdCompressSync(big)).length);\n",
            "console.log(zlib.unzipSync(zlib.gzipSync(big)).length, zlib.unzipSync(zlib.deflateSync(big)).length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "200000\n200000\n200000\n200000\n200000\n200000 200000");
}

/// 验证 stream/web 导出表面：三个构造器与 ReadableStreamTee 均为函数
///（同时覆盖无 `node:` 前缀的 `require("stream/web")`）。
#[test]
fn stream_web_exports_typeof_e2e_matches_go() {
    let work = work_dir("web_typeof");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const web = require(\"stream/web\");\n",
            "console.log(typeof web.ReadableStream, typeof web.WritableStream, ",
            "typeof web.TransformStream, typeof web.ReadableStreamTee);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "function function function function");
}

/// 验证 ReadableStream 实例表面：`locked` 初始 false、`getReader` 后置 true、
/// 方法属性存在性、`tee()` 返回同一对象身份的二元组。
#[test]
fn stream_web_readable_stream_surface_e2e_matches_go() {
    let work = work_dir("web_readable");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const web = require(\"node:stream/web\");\n",
            "const rs = new web.ReadableStream();\n",
            "console.log(typeof rs, rs.locked, typeof rs.getReader, typeof rs.tee, ",
            "typeof rs.cancel, typeof rs.enqueue, typeof rs.close, typeof rs.pipeTo);\n",
            "const t = web.ReadableStreamTee(rs);\n",
            "console.log(t[0] === rs, t[1] === rs, t.length);\n",
            "console.log(web.ReadableStreamTee());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "object false function function function function function function\ntrue true 2\nundefined"
    );
}

/// 验证 WritableStream / TransformStream 表面与模块级构造器身份一致性。
#[test]
fn stream_web_writable_transform_e2e_matches_go() {
    let work = work_dir("web_writable");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const web = require(\"node:stream/web\");\n",
            "const ws = new web.WritableStream();\n",
            "console.log(typeof ws, typeof ws.getWriter, typeof ws.write, typeof ws.close);\n",
            "const ts = new web.TransformStream();\n",
            "console.log(typeof ts, typeof ts.readable, typeof ts.writable, ",
            "typeof ts.readable.getReader, typeof ts.writable.getWriter);\n",
            "console.log(web.ReadableStream === web.ReadableStream, web.TransformStream === web.TransformStream);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "object function function function\nobject object object function function\ntrue true"
    );
}

/// 验证 ReadableStreamTee 的完整分派语义：reader 表面（read/cancel/releaseLock）、
/// 无 tee 方法的普通对象回退 `[obj, obj]`、带 tee 方法的对象直接调用。
#[test]
fn stream_web_tee_reader_e2e_matches_go() {
    let work = work_dir("web_tee");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const web = require(\"node:stream/web\");\n",
            "const rs = new web.ReadableStream();\n",
            "const reader = rs.getReader();\n",
            "console.log(typeof reader, typeof reader.read, typeof reader.cancel, ",
            "typeof reader.releaseLock, rs.locked);\n",
            "const plain = { v: 1 };\n",
            "const t2 = web.ReadableStreamTee(plain);\n",
            "console.log(typeof t2, t2[0] === plain, t2[1] === plain);\n",
            "const t3 = web.ReadableStreamTee({ tee: function() { return \"teed\"; } });\n",
            "console.log(t3);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "object function function function true\nobject true true\nteed"
    );
}

/// 验证 ReadableStream 数据面：`enqueue` 入队、`read()` 经 Promise 取出队首、
/// `close()` 后队列仍优先消费（对齐 Go gstream 队列语义）；同时覆盖
/// `ReadableStreamTee` 对非对象实参抛错的分支（只对拍「是否抛出」）。
#[test]
fn stream_web_read_cycle_e2e_matches_go() {
    let work = work_dir("web_read_cycle");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const web = require(\"node:stream/web\");\n",
            "try { web.ReadableStreamTee(42); console.log(\"no throw\"); } ",
            "catch (e) { console.log(\"caught tee\"); }\n",
            "async function main() {\n",
            "    const rs = new web.ReadableStream();\n",
            "    rs.enqueue(\"q1\");\n",
            "    rs.enqueue(\"q2\");\n",
            "    const reader = rs.getReader();\n",
            "    const r1 = await reader.read();\n",
            "    console.log(\"read1:\", r1.value, r1.done);\n",
            "    rs.close();\n",
            "    const r2 = await reader.read();\n",
            "    console.log(\"read2:\", r2.value, r2.done);\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "caught tee\nread1: q1 false\nread2: q2 false");
}
