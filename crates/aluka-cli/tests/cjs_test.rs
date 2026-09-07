//! CJS 模块系统端到端测试：Rust 前端编译多模块 + 循环依赖 + fs IO，
//! `aluvm` 执行并与 Node.js 22 LTS 权威标准对拍。
//!
//! 字节码分发约定：`require("./x")` 解析为入口目录下的 `x.bc`
//! （`.js`/无后缀 → `.bc` 替换/补全）。

use std::path::{Path, PathBuf};
use std::process::Command;

fn aluvm_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_aluvm")).to_path_buf()
}

fn alukac_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_alukac")).to_path_buf()
}

fn node_bin() -> Option<String> {
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_string());
    if Command::new(&node)
        .arg("-v")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        Some(node)
    } else {
        None
    }
}

/// 编译目录下所有 JS 文件为对应同名 .bc
fn compile_all_modules(dir: &Path) {
    let compiler = alukac_exe();
    for entry in std::fs::read_dir(dir).expect("读取工作目录失败").flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|ext| ext == "js") {
            let bc = p.with_extension("bc");
            let out = Command::new(&compiler)
                .arg("compile")
                .arg(&p)
                .arg("-o")
                .arg(&bc)
                .output()
                .expect("编译模块失败");
            assert!(
                out.status.success(),
                "alukac 编译 {:?} 失败: {}",
                p,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn aluvm_cjs_modules_cycle_and_fs_end_to_end() {
    let work = std::env::temp_dir().join(format!("aluvm_cjs_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("创建工作目录");

    // 四模块应用：CJS 基本流 + 循环依赖 + fs IO
    std::fs::write(
        work.join("app.js"),
        concat!(
            "const dep = require(\"./dep.js\");\n",
            "const loop = require(\"./loop-a.js\");\n",
            "const fs = require(\"fs\");\n",
            "fs.writeFileSync(\"m1_io.txt\", \"written-by-aluvm\");\n",
            "const back = fs.readFileSync(\"m1_io.txt\", \"utf-8\");\n",
            "console.log(\"main got:\", dep.value);\n",
            "console.log(\"loop:\", loop.tag);\n",
            "console.log(\"fs:\", back);\n",
        ),
    )
    .unwrap();
    std::fs::write(
        work.join("dep.js"),
        "console.log(\"dep loading\");\nexports.value = 42;\n",
    )
    .unwrap();
    std::fs::write(
        work.join("loop-a.js"),
        concat!(
            "const b = require(\"./loop-b.js\");\n",
            "exports.tag = \"A(\" + b.tag + \")\";\n",
        ),
    )
    .unwrap();
    std::fs::write(
        work.join("loop-b.js"),
        concat!(
            "const a = require(\"./loop-a.js\");\n",
            "exports.tag = \"B\";\n",
            "console.log(\"b sees a partially:\", typeof a.tag, a.tag === undefined);\n",
        ),
    )
    .unwrap();

    // 使用 Rust alukac 前端编译所有依赖模块
    compile_all_modules(&work);

    // Rust VM 执行入口模块
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(work.join("app.bc"))
        .current_dir(&work)
        .output()
        .expect("运行 aluvm 失败");
    assert!(
        out.status.success(),
        "CJS 应用应执行成功: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");

    // Node.js 22 LTS 同负载对照
    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg(work.join("app.js"))
            .current_dir(&work)
            .output()
            .expect("运行 Node.js 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .trim()
            .replace("\r\n", "\n");

        println!("Rust:\n{rust_out}\nNode:\n{node_stdout}");
        assert_eq!(
            rust_out, node_stdout,
            "CJS 多模块 + 循环依赖 + fs IO 的输出必须与 Node.js 22 一致"
        );
    }

    // 关键语义断言
    assert!(rust_out.contains("main got: 42"), "exports 传递");
    assert!(rust_out.contains("loop: A(B)"), "循环依赖完成态");
    assert!(
        rust_out.contains("b sees a partially: undefined true"),
        "循环依赖中后加载方持有未完成 exports"
    );
    assert!(rust_out.contains("fs: written-by-aluvm"), "fs 同步读写");

    // fs 落盘验证
    let io = std::fs::read_to_string(work.join("m1_io.txt")).expect("fs 产物读取");
    assert_eq!(io, "written-by-aluvm");
}

/// `node:path` / `fs.existsSync` / `process.env` 轻量内置端到端。
#[test]
fn aluvm_node_path_fs_env_builtins_e2e() {
    let work = std::env::temp_dir().join(format!("aluvm_builtins_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("创建工作目录");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const path = require(\"path\");\n",
            "console.log(\"j1:\" + path.join(\"a\", \"b\"));\n",
            "console.log(\"j2:\" + path.join(\"/x\", \"y\", \"z\"));\n",
            "console.log(\"b1:\" + path.basename(\"/a/b/file.txt\"));\n",
            "console.log(\"b2:\" + path.basename(\"/a/b/file.txt\", \".txt\"));\n",
            "console.log(\"d1:\" + path.dirname(\"/a/b/file.txt\"));\n",
            "console.log(\"e1:\" + path.extname(\"/a/b/file.txt\"));\n",
            "const fs = require(\"fs\");\n",
            "console.log(\"ex:\" + fs.existsSync(\"probe.js\"));\n",
            "console.log(\"no:\" + fs.existsSync(\"definitely_missing_xyz\"));\n",
            "console.log(\"env:\" + (process.env.PATH ? \"yes\" : \"no\"));\n",
        ),
    )
    .unwrap();

    compile_all_modules(&work);

    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(work.join("probe.bc"))
        .current_dir(&work)
        .output()
        .expect("运行 aluvm 失败");
    assert!(
        out.status.success(),
        "内置库用例应成功: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");

    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg(work.join("probe.js"))
            .current_dir(&work)
            .output()
            .expect("运行 Node.js 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .trim()
            .replace("\r\n", "\n");
        println!("Rust:\n{rust_out}\nNode:\n{node_stdout}");
        assert_eq!(
            rust_out.replace('\\', "/"),
            node_stdout.replace('\\', "/"),
            "path/fs/env 内置输出必须与 Node.js 22 一致"
        );
    }
}

/// `os` / `new URL(...)` 轻量内置端到端。
#[test]
fn aluvm_os_and_url_builtins_e2e() {
    let work = std::env::temp_dir().join(format!("aluvm_osurl_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("创建工作目录");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const os = require(\"os\");\n",
            "console.log(\"p:\" + os.platform());\n",
            "console.log(\"h:\" + os.homedir());\n",
            "console.log(\"t:\" + os.tmpdir());\n",
            "const u = new URL(\"https://user:pass@example.com:8080/p/q?a=1&b=2#frag\");\n",
            "console.log(\"q:\" + u.protocol + \"|\" + u.hostname + \"|\" + u.port + \"|\" + u.pathname + \"|\" + u.search + \"|\" + u.hash + \"|\" + u.href);\n",
            "console.log(\"s:\" + u.host + \"|\" + u.origin);\n",
        ),
    )
    .unwrap();

    compile_all_modules(&work);

    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(work.join("probe.bc"))
        .current_dir(&work)
        .output()
        .expect("运行 aluvm 失败");
    assert!(
        out.status.success(),
        "os/url 用例应成功: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");

    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg(work.join("probe.js"))
            .current_dir(&work)
            .output()
            .expect("运行 Node.js 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .trim()
            .replace("\r\n", "\n");
        println!("Rust:\n{rust_out}\nNode:\n{node_stdout}");
        assert_eq!(
            rust_out, node_stdout,
            "os/url 内置输出必须与 Node.js 22 一致"
        );
    }
}
