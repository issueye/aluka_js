//! 内置库 Phase 1（T1 轻量模块）端到端测试：`path/posix`、`path/win32`、
//! `constants`、`string_decoder`。
//!
//! 全部经 `common::assert_e2e_matches_go` 与 Node.js 22 LTS 标准 逐字对拍：
//! 源文件写临时目录 → Go 前端整图编译 → aluvm 执行 → Node.js 22 LTS 标准 同负载
//! 输出比对。对拍口径来自 Go 侧 `nodeos`/`nodestream` 实测：
//! - path 系对齐 Go `path` / `path/filepath`(Windows) 语义（含 `basename("")=“.”`
//!   等 oracle 特有行为）；
//! - constants 对齐 `constants_data.go` 常量表（Windows 信号集 11 项）；
//! - string_decoder 对齐 `utf8ValidPrefix`（完整多字节字符也会被暂存）。

mod common;

use std::path::PathBuf;

fn work_dir(tag: &str) -> PathBuf {
    let pid = std::process::id();
    std::env::temp_dir().join(format!("aluvm_phase1_{tag}_{pid}"))
}

fn write_probe(work: &PathBuf, js: &str) {
    std::fs::create_dir_all(work).expect("创建工作目录");
    std::fs::write(work.join("probe.js"), js).expect("写入 probe.js");
}

/// `path/posix`：join/basename/dirname/extname/resolve（POSIX `/` 语义 + 边界）。
#[test]
fn phase1_path_posix_e2e() {
    let work = work_dir("path_posix");
    write_probe(
        &work,
        r#"const p = require("path/posix");
const q = require("node:path/posix");
console.log("same:" + (p === q));
console.log("j1:" + p.join("a", "b"));
console.log("j2:" + p.join("/x", "y", "z"));
console.log("j3:" + p.join("a", "", "b"));
console.log("j4:" + p.join());
console.log("j5:" + p.join(".", "b"));
console.log("j6:" + p.join("a", "..", "b"));
console.log("j7:" + p.join("/a", "/b"));
console.log("j8:" + p.join("a/", "b/"));
console.log("j9:" + p.join("a", "bc", ".."));
console.log("j10:" + p.join("..", ".."));
console.log("j11:" + p.join("/..", "x"));
console.log("j12:" + p.join(".", "."));
console.log("b1:" + p.basename("/a/b/file.txt"));
console.log("b2:" + p.basename("/a/b/file.txt", ".txt"));
console.log("b3:" + p.basename("/a/b/"));
console.log("b4:" + p.basename(""));
console.log("b5:" + p.basename("/"));
console.log("b6:" + p.basename("/a/b/c.d", ".d"));
console.log("b7:" + p.basename("/a/b/c.d", "z"));
console.log("b8:" + p.basename("a"));
console.log("d1:" + p.dirname("/a/b/file.txt"));
console.log("d2:" + p.dirname("file.txt"));
console.log("d3:" + p.dirname("/"));
console.log("d4:" + p.dirname(""));
console.log("d5:" + p.dirname("a/b/"));
console.log("d6:" + p.dirname("/a/b/"));
console.log("e1:" + p.extname("file.txt"));
console.log("e2:" + p.extname(".bashrc"));
console.log("e3:" + p.extname("file.tar.gz"));
console.log("e4:" + p.extname("file."));
console.log("e5:" + p.extname("noext"));
console.log("e6:" + p.extname("/a.b/file"));
console.log("r1:" + p.resolve("/a", "b", "../c"));
console.log("r2:" + p.resolve("a", "b"));
console.log("r3:" + p.resolve());
console.log("r4:" + p.resolve("..", "x"));
console.log("r5:" + p.resolve("/"));
console.log("r6:" + p.resolve("", "z"));
"#,
    );
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    // 关键语义抽查（逐字对拍已保证整体一致）
    assert!(out.contains("j2:/x/y/z"), "绝对路径 join");
    assert!(out.contains("b2:file"), "basename 去扩展名二参");
    assert!(
        out.contains("b4:."),
        "basename 空串 = .（Node.js 22 LTS 标准 口径）"
    );
    assert!(out.contains("d2:."), "dirname 无分隔符 = .");
    assert!(out.contains("e2:"), "extname 隐藏文件为空");
    assert!(out.contains("r1:/a/c"), "resolve 绝对化 + .. 消解");
}

/// `path/win32`：`\` 分隔符语义（`/` 等效输入；join/dirname 输出恒为 `\`）。
#[test]
fn phase1_path_win32_e2e() {
    let work = work_dir("path_win32");
    write_probe(
        &work,
        r#"const w = require("path/win32");
const w2 = require("node:path/win32");
console.log("same:" + (w === w2));
console.log("wj1:" + w.join("a", "b"));
console.log("wj2:" + w.join("C:/a", "b"));
console.log("wj3:" + w.join("a/b", "c"));
console.log("wj4:" + w.join("C:", "foo"));
console.log("wj5:" + w.join("C:/", "foo"));
console.log("wj6:" + w.join("/x", "y"));
console.log("wj7:" + w.join("a", "..", "b"));
console.log("wj8:" + w.join());
console.log("wj9:" + w.join("a", "", "b"));
console.log("wj10:" + w.join("C:", "/f"));
console.log("wj11:" + w.join("C:/a/", "b"));
console.log("wj12:" + w.join("a", "b", ""));
console.log("wb1:" + w.basename("C:/a/b/file.txt"));
console.log("wb2:" + w.basename("C:/a/b/file.txt", ".txt"));
console.log("wb3:" + w.basename("C:/a/b/"));
console.log("wb4:" + w.basename("a/b/c"));
console.log("wb5:" + w.basename("C:/"));
console.log("wb6:" + w.basename(""));
console.log("wb7:" + w.basename("C:"));
console.log("wb8:" + w.basename("."));
console.log("wb9:" + w.basename("a/b/c.d", ".d"));
console.log("wd1:" + w.dirname("C:/a/b/file.txt"));
console.log("wd2:" + w.dirname("C:/a/b/"));
console.log("wd3:" + w.dirname("file.txt"));
console.log("wd4:" + w.dirname("C:/file.txt"));
console.log("wd5:" + w.dirname("C:foo"));
console.log("wd6:" + w.dirname("a/b/c"));
console.log("wd7:" + w.dirname(""));
console.log("wd8:" + w.dirname("C:/"));
console.log("we1:" + w.extname("C:/a/b/file.txt"));
console.log("we2:" + w.extname("file.tar.gz"));
console.log("we3:" + w.extname(".bashrc"));
console.log("we4:" + w.extname("file."));
console.log("wr1:" + w.resolve("C:/a", "b"));
console.log("wr2:" + w.resolve("a", "b"));
console.log("wr3:" + w.resolve());
console.log("wr4:" + w.resolve("C:", "b"));
console.log("wr5:" + w.resolve("C:", "/f"));
console.log("wr6:" + w.resolve("/x", "y"));
console.log("wr7:" + w.resolve("C:/"));
console.log("wr8:" + w.resolve("a", "..", "b"));
"#,
    );
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("wj1:a\\b"), "win32 join 输出反斜杠");
    assert!(out.contains("wj4:C:foo"), "驱动相对不加分隔符");
    assert!(out.contains("wb3:b"), "basename 去尾部斜杠");
    assert!(out.contains("wb6:."), "basename 空串 = .");
    assert!(out.contains("wd5:C:."), "dirname 驱动相对 = C:.");
    assert!(out.contains("we3:"), "extname 隐藏文件为空");
    assert!(out.contains("wr1:C:\\a\\b"), "resolve 绝对输入直接 Clean");
    assert!(out.contains("wr4:"), "resolve 驱动相对走同驱动 cwd");
}

/// `constants`：信号/errno/priority/uv/fs/openssl 常量（含字符串常量）。
#[test]
fn phase1_constants_e2e() {
    let work = work_dir("constants");
    write_probe(
        &work,
        r#"const c = require("constants");
const c2 = require("node:constants");
console.log("same:" + (c === c2));
console.log("sig:" + c.SIGINT + "|" + c.SIGTERM + "|" + c.SIGHUP + "|" + c.SIGKILL + "|" + c.SIGSEGV + "|" + c.SIGABRT + "|" + c.SIGBREAK + "|" + c.SIGFPE + "|" + c.SIGILL + "|" + c.SIGQUIT + "|" + c.SIGWINCH);
console.log("pri:" + c.PRIORITY_ABOVE_NORMAL + "|" + c.PRIORITY_BELOW_NORMAL + "|" + c.PRIORITY_HIGH + "|" + c.PRIORITY_HIGHEST + "|" + c.PRIORITY_LOW + "|" + c.PRIORITY_NORMAL);
console.log("err:" + c.ENOENT + "|" + c.EACCES + "|" + c.EPERM + "|" + c.EINVAL + "|" + c.ENOTDIR + "|" + c.EXDEV);
console.log("o:" + c.O_CREAT + "|" + c.O_RDWR + "|" + c.O_APPEND + "|" + c.F_OK + "|" + c.W_OK + "|" + c.X_OK);
console.log("s:" + c.S_IFDIR + "|" + c.S_IFREG + "|" + c.S_IFCHR + "|" + c.S_IFMT + "|" + c.S_IRUSR + "|" + c.S_IWUSR);
console.log("uv:" + c.UV_FS_COPYFILE_FICLONE + "|" + c.UV_FS_COPYFILE_EXCL + "|" + c.UV_DIRENT_UNKNOWN + "|" + c.UV_DIRENT_DIR + "|" + c.UV_FS_SYMLINK_JUNCTION);
console.log("tls:" + c.TLS1_VERSION + "|" + c.TLS1_1_VERSION + "|" + c.TLS1_2_VERSION + "|" + c.TLS1_3_VERSION);
console.log("ssl:" + c.SSL_OP_ALL + "|" + c.SSL_OP_NO_TLSv1_3 + "|" + c.OPENSSL_VERSION_NUMBER);
console.log("engine:" + c.ENGINE_METHOD_ALL + "|" + c.ENGINE_METHOD_RSA + "|" + c.ENGINE_METHOD_NONE);
console.log("wsa:" + c.WSAEACCES + "|" + c.WSAECONNRESET + "|" + c.WSAETIMEDOUT);
console.log("cipher:" + (typeof c.defaultCoreCipherList) + ":" + c.defaultCoreCipherList);
"#,
    );
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("sig:2|15|1|9|11|22|21|8|4|3|28"),
        "Windows 11 信号常量（Node.js 22 LTS 标准 实测值）"
    );
    assert!(out.contains("pri:-7|10|-14|-20|19|0"), "优先级常量含负值");
    assert!(out.contains("s:16384|32768|8192|61440|256|128"));
    assert!(out.starts_with("same:true\nsig:"), "节点前缀别名同一单例");
    assert!(
        out.ends_with(
            "DHE-RSA-AES256-SHA256:HIGH:!aNULL:!eNULL:!EXPORT:!DES:!RC4:!MD5:!PSK:!SRP:!CAMELLIA"
        ),
        "字符串常量值完整对拍"
    );
}

/// `string_decoder`：`StringDecoder()` 的 write/end 增量 UTF-8 解码
/// （Go 语义：完整多字节字符也会暂存，直到后续起始字节闭合）。
#[test]
fn phase1_string_decoder_e2e() {
    let work = work_dir("string_decoder");
    write_probe(
        &work,
        r#"const sd = require("string_decoder");
const d = sd.StringDecoder();
console.log("r1:" + d.write("abc"));
console.log("r2:" + d.write("def"));
console.log("r3:" + d.end());
const d2 = sd.StringDecoder();
console.log("r4:" + d2.write("中"));
console.log("r5:" + d2.end());
const d3 = sd.StringDecoder("utf8");
console.log("r6:" + d3.encoding);
console.log("r7:" + d3.write("a中b"));
console.log("r8:" + d3.write("c"));
console.log("r9:" + d3.end());
const d4 = sd.StringDecoder();
console.log("r10:" + d4.write("x"));
console.log("r11:" + d4.write("中"));
console.log("r12:" + d4.write("x"));
console.log("r13:" + d4.end());
const d5 = sd.StringDecoder();
console.log("r14:" + d5.write("a"));
console.log("r15:" + d5.end("bc"));
console.log("r16:" + d5.end());
const d6 = sd.StringDecoder();
console.log("r17:" + d6.write());
console.log("r18:" + d6.write("z"));
const d7 = sd.StringDecoder();
console.log("r19:" + d7.write("abc"));
console.log("r20:" + d7.write(""));
console.log("r21:" + d7.end());
console.log("r22:" + d7.write("gh"));
"#,
    );
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("r1:abc"), "ASCII 直接解码");
    assert!(out.contains("r4:"), "完整多字节字符被 Go 语义暂存");
    assert!(out.contains("r5:中"), "end() 刷新暂存");
    assert!(out.contains("r7:a中b"), "以起始字节收尾时整体吐出");
    assert!(out.contains("r8:c"), "暂存被冲刷后新写入直接返回");
    assert!(out.contains("r12:中x"), "暂存+后续 ASCII 闭合后一起输出");
    assert!(out.contains("r15:bc"), "end(chunk) 合并输出");
    assert!(
        out.contains("r19:abc\nr20:\nr21:"),
        "write(\"\") 空状态返回空串、end() 无残留"
    );
}
