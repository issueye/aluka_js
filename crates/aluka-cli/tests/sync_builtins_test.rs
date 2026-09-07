//! Phase 2 同步族内置库 e2e 对拍：`fs`（readdirSync/statSync/mkdirSync/rmSync）、
//! `os`（arch/release/type/cpus/userInfo）、`util`（format/inspect/util.types）、
//! `assert`（ok/equal/strictEqual/throws）。
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。

mod common;

use std::path::PathBuf;

/// 每用例独立工作目录（先清空，保证三次执行与跨次运行幂等）。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sync_builtins_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录");
    dir
}

/// `fs` 同步族：readdirSync（排序数组）/ statSync（isFile/isDirectory/size/
/// mtimeMs）/ mkdirSync / rmSync（{recursive}），文件树操作脚本风格。
#[test]
fn fs_sync_family_e2e_matches_go() {
    let work = work_dir("fs");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const fs = require(\"fs\");\n",
            "fs.rmSync(\"d1\", { recursive: true });\n",
            "fs.mkdirSync(\"d1\");\n",
            "fs.writeFileSync(\"d1/a.txt\", \"hello\");\n",
            "fs.writeFileSync(\"d1/b.txt\", \"world\");\n",
            "const names = fs.readdirSync(\"d1\");\n",
            "console.log(\"names:\", names.length, names[0], names[1], names.join(\",\"));\n",
            "const s1 = fs.statSync(\"d1\");\n",
            "console.log(\"dir:\", s1.isDirectory(), s1.isFile(), s1.size);\n",
            "const s2 = fs.statSync(\"d1/a.txt\");\n",
            "console.log(\"file:\", s2.isFile(), s2.isDirectory(), s2.size);\n",
            "console.log(\"mtime:\", typeof s2.mtimeMs, s2.mtimeMs > 0);\n",
            "fs.rmSync(\"d1/a.txt\");\n",
            "console.log(\"deleted:\", fs.existsSync(\"d1/a.txt\"));\n",
            "fs.rmSync(\"d1\", { recursive: true });\n",
            "console.log(\"gone:\", !fs.existsSync(\"d1\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "names: 2 a.txt b.txt a.txt,b.txt\ndir: true false 0\nfile: true false 5\nmtime: number true\ndeleted: false\ngone: true"
    );
}

/// `os` 扩展：arch/release/type/cpus（单 CPU 形态）/userInfo().username，
/// 与既有 platform/homedir/tmpdir 共存。
///
/// 说明：解释器在 `register_all` 之后才创建 os 单例，首个注册表方法调用会
/// 完成惰性重链（见 `builtins/os.rs`），故探测先调 `util.format("")` 一次。
#[test]
fn os_extended_family_e2e_matches_go() {
    let work = work_dir("os");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const util = require(\"util\");\n",
            "util.format(\"\");\n",
            "const os = require(\"os\");\n",
            "console.log(\"arch:\", os.arch());\n",
            "console.log(\"type:\", os.type());\n",
            "console.log(\"release:\", os.release(), typeof os.release());\n",
            "const c = os.cpus();\n",
            "console.log(\"cpus:\", typeof c, c.length >= 1, c[0].model, c[0].speed, typeof c[0].times);\n",
            "console.log(\"times:\", c[0].times.user, c[0].times.nice, c[0].times.sys, c[0].times.idle, c[0].times.irq);\n",
            "const u = os.userInfo();\n",
            "console.log(\"user:\", u.username, typeof u.homedir);\n",
            "console.log(\"plat:\", os.platform(), os.homedir() !== \"\", os.tmpdir() !== \"\");\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    // 平台相关值逐字对拍由 helper 保证；此处再锚定输出行数，便于失败定位。
    assert_eq!(out.lines().count(), 7);
}

/// `util`：format（%s/%d/%j/%% 与无占位符空格连接）、inspect（字符串/数组/
/// 普通对象紧凑表示）、util.types 类型判断（isArray/isString/isNumber/isObject）。
#[test]
fn util_format_inspect_types_e2e_matches_go() {
    let work = work_dir("util");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const util = require(\"util\");\n",
            "console.log(\"f1:\", util.format(\"%s %d %j\", \"abc\", 42, {a: 1}));\n",
            "console.log(\"f2:\", util.format(\"%s|%d|%j|%%\", \"x\", 3.5, [1, 2]));\n",
            "console.log(\"f3:\", util.format(\"no placeholder\", 1, 2));\n",
            "console.log(\"f4:\", util.format(\"%s\"));\n",
            "console.log(\"f5:\", util.format(\"a %s b %s\", \"mid\"));\n",
            "console.log(\"i1:\", util.inspect(\"abc\"));\n",
            "console.log(\"i2:\", util.inspect(42));\n",
            "console.log(\"i3:\", util.inspect([1, \"x\", true]));\n",
            "console.log(\"i4:\", util.inspect({a: 1, b: \"yz\"}));\n",
            "console.log(\"i5:\", util.inspect(null), util.inspect(undefined), util.inspect(true));\n",
            "console.log(\"t1:\", util.types.isArray([]), util.types.isArray({}));\n",
            "console.log(\"t2:\", util.types.isString(\"s\"), util.types.isString(1));\n",
            "console.log(\"t3:\", util.types.isNumber(1), util.types.isNumber(\"1\"));\n",
            "console.log(\"t4:\", util.types.isObject({}), util.types.isObject(null));\n",
            "console.log(\"d1:\", typeof util.isArray);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "f1: abc 42 { a: 1 }\n",
            "f2: x|3|[ 1, 2 ]|%\n",
            "f3: no placeholder 1 2\n",
            "f4: \n",
            "f5: a mid b \n",
            "i1: abc\n",
            "i2: 42\n",
            "i3: [ 1, x, true ]\n",
            "i4: { a: 1, b: yz }\n",
            "i5: null undefined true\n",
            "t1: true false\n",
            "t2: true false\n",
            "t3: true false\n",
            "t4: true false\n",
            "d1: undefined"
        )
    );
}

/// `assert`：ok/equal/strictEqual 通过路径 + throws 捕获（Error 实例与字符串
/// 两种抛出形态），断言失败不触发（探测脚本须在 Go 前端编译期也全部通过）。
#[test]
fn assert_family_e2e_matches_go() {
    let work = work_dir("assert");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const assert = require(\"assert\");\n",
            "assert.ok(true);\n",
            "assert.ok(1);\n",
            "console.log(\"ok: passed\");\n",
            "assert.equal(1, 1);\n",
            "assert.equal(\"a\", \"a\");\n",
            "assert.equal(1, \"1\");\n",
            "console.log(\"equal: passed\");\n",
            "assert.strictEqual(\"x\", \"x\");\n",
            "assert.strictEqual(42, 42);\n",
            "console.log(\"strictEqual: passed\");\n",
            "assert.throws(() => { throw new Error(\"boom\"); });\n",
            "assert.throws(() => { throw \"str-err\"; });\n",
            "console.log(\"throws: passed\");\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "ok: passed\n",
            "equal: passed\n",
            "strictEqual: passed\n",
            "throws: passed"
        )
    );
}
