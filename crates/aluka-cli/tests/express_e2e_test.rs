//! M2.4 Express 真实依赖树端到端：6 大核心场景与 Node.js 22 oracle 对拍。
//!
//! fixture 依赖 `demo/express-demo` 的本地 `node_modules`（express@4 全树，
//! 由 `npm ci` 安装、不入库）——`node_modules` 缺失时本测试跳过（CI 等
//! 未安装依赖的环境不阻塞门禁）。
//!
//! 流程：`alukac build` 全树预编译 → `aluvm run` 执行入口 →
//! stdout 与固化 oracle 行逐条断言，并（Node 可用时）与 `node app.js`
//! 实时对拍。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 仓库根：`crates/aluka-cli` 上溯两级（aluka-cli → crates → 仓库根）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn demo_dir() -> PathBuf {
    repo_root().join("demo").join("express-demo")
}

fn alukac_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_alukac")).to_path_buf()
}

fn aluvm_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_aluvm")).to_path_buf()
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

/// fixture 就绪：express 已安装才跑（npm ci 后 68 包）。
fn fixture_ready() -> bool {
    demo_dir().join("node_modules").join("express").is_dir()
}

/// 固化 oracle（与 `demo/express-demo/app.oracle.txt` 同源，逐行断言
/// 防 oracle 文件被误改后测试静默漂移）。
const ORACLE_LINES: &[&str] = &[
    "PORT_READY",
    "GET / -> 200 hello from express",
    "ECHO -> 200 echo: world",
    "POST -> 200 {\"got\":{\"n\":1}}",
    "CONCURRENT -> 200,200,200",
    "CTYPE -> 200 application/xml; charset=utf-8 | <root>ok</root>",
    "CLOSED",
];

/// M2.4 验收：Express 真实依赖树 6 大核心场景（GET / · echo/:word ·
/// POST /json · 并发 · 自定义 Content-Type · 优雅退出）与 Node 22 对拍。
#[test]
fn express_six_scenes_match_node22_oracle() {
    let demo = demo_dir();
    if !fixture_ready() {
        eprintln!("skip: demo/express-demo/node_modules 缺失（需 `npm ci`，fixture 不入库）");
        return;
    }

    // 1. 全树预编译（入口 app.js 的 require 闭包，132 模块级）
    let build = Command::new(alukac_exe())
        .arg("build")
        .arg("app.js")
        .current_dir(&demo)
        .output()
        .expect("alukac build 失败");
    assert!(
        build.status.success(),
        "alukac build 失败: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // 2. aluvm 执行入口字节码
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(demo.join("aluka_build").join("app.bc"))
        .current_dir(&demo)
        .output()
        .expect("aluvm 运行失败");
    assert!(
        out.status.success(),
        "aluvm 运行失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_owned();

    // 3. 固化 oracle 逐行断言（6 场景全绿）
    let got: Vec<&str> = rust_out.lines().collect();
    assert_eq!(
        got, ORACLE_LINES,
        "Express 6 场景输出必须与 Node 22 oracle 逐行一致"
    );

    // 4. Node 22 实时对拍（权威 oracle 在场时）
    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg("app.js")
            .current_dir(&demo)
            .output()
            .expect("运行 Node.js 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .replace("\r\n", "\n")
            .trim()
            .to_owned();
        println!("Aluka:\n{rust_out}\nNode:\n{node_stdout}");
        assert_eq!(rust_out, node_stdout, "Express 输出必须与 Node.js 22 一致");
    }
}
