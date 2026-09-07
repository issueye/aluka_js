//! 内置库全量加载审计（builtins-manifest.md 60 模块矩阵的收口证明）：
//! 对清单中全部可 `require` 的内置模块逐一加载，aluvm 与 Node.js 22 LTS 标准 必须
//! 输出完全一致（loaded 计数与 failed 列表逐字相同）。
//!
//! `process`/`console`/`url` 为全局对象亦可 require；`process.getBuiltinModule`
//! 是 API 不是模块，不在 require 清单内（其行为由 cjs_test 覆盖）。

mod common;

use std::path::PathBuf;

const MODULES: &[&str] = &[
    "buffer",
    "timers",
    "timers/promises",
    "perf_hooks",
    "v8",
    "fs",
    "fs/promises",
    "os",
    "util",
    "util/types",
    "assert",
    "assert/strict",
    "path",
    "path/posix",
    "path/win32",
    "querystring",
    "string_decoder",
    "constants",
    "process",
    "console",
    "url",
    "events",
    "stream",
    "stream/web",
    "stream/promises",
    "stream/consumers",
    "crypto",
    "zlib",
    "http",
    "https",
    "net",
    "tls",
    "dns",
    "dns/promises",
    "dgram",
    "http2",
    "child_process",
    "worker_threads",
    "cluster",
    "vm",
    "diagnostics_channel",
    "async_hooks",
    "inspector",
    "inspector/promises",
    "trace_events",
    "readline",
    "readline/promises",
    "repl",
    "tty",
    "sqlite",
    "domain",
    "punycode",
    "wasi",
    "test",
    "test/reporters",
    "module",
    "sys",
    "markdown",
    "aluka:markdown",
];

/// 全部内置模块在 aluvm 与 Node.js 22 LTS 标准 两侧均可 require 且数量一致。
#[test]
fn all_builtin_modules_requireable_e2e_matches_go() {
    let work = std::env::temp_dir().join(format!("builtins_all_modules_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");

    let mut js = String::from("const mods = [\n");
    for m in MODULES {
        js.push_str(&format!("    \"{m}\",\n"));
    }
    js.push_str(
        "];\n\
         let ok = 0;\n\
         let failed = \"\";\n\
         for (let i = 0; i < mods.length; i++) {\n\
         \x20   try {\n\
         \x20       require(mods[i]);\n\
         \x20       ok++;\n\
         \x20   } catch (e) {\n\
         \x20       failed = failed + (failed === \"\" ? \"\" : \",\") + mods[i];\n\
         \x20   }\n\
         }\n\
         console.log(\"loaded:\", ok);\n\
         console.log(\"failed:\", failed);\n\
         const c = require(\"process\").getBuiltinModule(\"crypto\");\n\
         console.log(\"getBuiltinModule:\", typeof c === \"object\" ? \"object\" : typeof c);\n",
    );
    std::fs::write(work.join("probe.js"), js).unwrap();

    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = format!(
        "loaded: {}\nfailed: \ngetBuiltinModule: object",
        MODULES.len()
    );
    assert_eq!(out, expected, "全部内置模块必须两侧一致可加载");
}

/// 测试清单与 manifest 矩阵条目数锚定（防漂移：新增模块必须同步加入审计）。
#[test]
fn manifest_module_count_anchor() {
    // manifest 矩阵 60 行 = 59 个可 require 模块 + process.getBuiltinModule API
    assert_eq!(MODULES.len(), 59);
}

/// 隔离临时目录辅助（对齐其他 phase 测试文件风格）。
#[allow(dead_code)]
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_all_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}
