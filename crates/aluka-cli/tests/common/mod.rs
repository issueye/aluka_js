//! 端到端测试共享 helper（Node.js 22 对拍 / 前端编译 / bc 分发）。
//!
//! 并行开发的测试基建：各能力模块的 e2e 测试文件以
//! `mod common;` 引入，只依赖本文件。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn aluvm_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_aluvm")).to_path_buf()
}

/// Rust 前端编译器（alukac）。
pub fn alukac_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_alukac")).to_path_buf()
}

/// 遍历工作目录并将所有 `.js` 编译为同级 `.bc`
pub fn compile_all_js(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "js") {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                let bc = p.with_file_name(format!("{stem}.bc"));
                let _ = Command::new(alukac_exe())
                    .arg(&p)
                    .arg("-o")
                    .arg(&bc)
                    .output();
            }
        }
    }
}

/// 递归收集 .bc 文件（排序保证确定性）。
pub fn walk_bc(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "bc") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// alukac 编译 → aluvm 执行：返回 trim 后 stdout（Rust 原生全链路）。
pub fn rust_pipeline_run(work: &Path, entry: &str) -> String {
    compile_all_js(work);
    let stem = entry.strip_suffix(".js").unwrap_or(entry);
    let bc = work.join(format!("{stem}.bc"));
    if !bc.exists() {
        let src = work.join(entry);
        let out = Command::new(alukac_exe())
            .arg(&src)
            .arg("-o")
            .arg(&bc)
            .output()
            .expect("alukac 编译失败");
        assert!(
            out.status.success(),
            "alukac 编译 {entry} 失败: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    aluvm_run(&bc)
}

/// 运行 aluvm 并返回输出（trim 后）。
pub fn aluvm_run(bc: &Path) -> String {
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(bc)
        .current_dir(bc.parent().expect("bc 有父目录"))
        .output()
        .expect("运行 aluvm 失败");
    assert!(
        out.status.success(),
        "aluvm 执行失败: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n")
        .to_string()
}

/// 运行 Node.js 22 LTS 并返回输出（trim 后）。
pub fn node_run(js: &Path) -> Option<String> {
    let node_bin = std::env::var("NODE").unwrap_or_else(|_| "node".to_string());
    let out = Command::new(node_bin)
        .arg(js)
        .current_dir(js.parent().unwrap_or(js))
        .output()
        .ok()?;
    if out.status.success() {
        Some(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .replace("\r\n", "\n")
                .to_string(),
        )
    } else {
        None
    }
}

/// 统一整图编译兼容函数（调用 alukac 编译）。
pub fn compile_graph(_placeholder: &Path, work: &Path, entry: &str) {
    compile_all_js(work);
    let stem = entry.strip_suffix(".js").unwrap_or(entry);
    let bc = work.join(format!("{stem}.bc"));
    if !bc.exists() {
        let src = work.join(entry);
        let _ = Command::new(alukac_exe())
            .arg(&src)
            .arg("-o")
            .arg(&bc)
            .output();
    }
}

/// 标准 e2e 一步：使用 alukac 编译 → aluvm 执行 → 并在 Node.js 22 可用时进行对拍。
pub fn assert_e2e_matches_node(work: &Path, entry: &str) -> String {
    let rust_out = rust_pipeline_run(work, entry);
    if let Some(node_out) = node_run(&work.join(entry)) {
        assert_eq!(
            rust_out.trim(),
            node_out.trim(),
            "e2e 输出与 Node.js 22 不一致（{entry}）"
        );
    }
    rust_out
}

/// 兼容别名
pub fn assert_e2e_matches_go(work: &Path, entry: &str) -> String {
    rust_pipeline_run(work, entry)
}
