//! 端到端测试共享 helper（Node.js 22 对拍 / 前端编译 / bc 分发）。
//!
//! 并行开发的测试基建：各能力模块的 e2e 测试文件以
//! `mod common;` 引入，只依赖本文件。

#![allow(dead_code)]

use std::io::Read;
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

/// e2e 子进程超时（默认 300s）。
///
/// 防御引擎性能回归（如字典属性写入退化 O(n²) 曾令 zlib 200KB 用例挂死
/// 19 分钟）：超时即 kill 并以显式 panic 失败，绝不静默挂死门禁。
pub const E2E_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// 带超时的子进程收尾：读取线程阻塞在管道 EOF（子进程退出即关闭 stdout/stderr），
/// 主线程用 `recv_timeout` 精确等待；超时 kill 并 panic。
///
/// **不用 `try_wait` + `sleep` 轮询**：Windows 默认定时器粒度 15.6ms，实测
/// `sleep(250ms)` 会让一个仅需约 19ms 的子进程白等约 250ms；本 helper 被 100+
/// 个 e2e 用例共享，这份白等会原样计入门禁墙钟（实测同一手法在 conformance
/// runner 上每子进程省一个定时器周期，见 conformance_node22_test.rs 的注释）。
fn finish_with_timeout(mut child: std::process::Child) -> std::process::Output {
    let stdout = child.stdout.take().expect("stdout 已 piped");
    let stderr = child.stderr.take().expect("stderr 已 piped");
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut out = stdout;
        let _ = out.read_to_end(&mut buf);
        let _ = out_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut err = stderr;
        let _ = err.read_to_end(&mut buf);
        let _ = err_tx.send(buf);
    });

    let deadline = std::time::Instant::now() + E2E_TIMEOUT;
    let mut timed_out = false;
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    for (rx, buf) in [(&out_rx, &mut stdout_buf), (&err_rx, &mut stderr_buf)] {
        match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            Ok(chunk) => buf.extend_from_slice(&chunk),
            // 超时：读取线程会在子进程被杀后随管道关闭自行退出（不再 join）
            Err(_) => timed_out = true,
        }
    }
    if timed_out {
        let _ = child.kill();
    }
    // 管道已 EOF ≈ 子进程已退出，wait() 立即返回；超时路径则在 kill 后收尸
    let status = child.wait().expect("等待子进程失败");
    if timed_out {
        panic!(
            "e2e 子进程超过 {:.0}s 未结束，已终止——疑似引擎性能回归或死锁",
            E2E_TIMEOUT.as_secs_f64()
        );
    }
    std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    }
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

/// 运行 aluvm 并返回输出（trim 后；带超时防护）。
pub fn aluvm_run(bc: &Path) -> String {
    let out = finish_with_timeout(
        Command::new(aluvm_exe())
            .arg("run")
            .arg(bc)
            .current_dir(bc.parent().expect("bc 有父目录"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("运行 aluvm 失败"),
    );
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

/// node 可执行文件名（可用 `NODE` 环境变量覆盖）。
fn node_bin() -> String {
    std::env::var("NODE").unwrap_or_else(|_| "node".to_string())
}

/// `node` 可执行文件是否存在且可启动（探测 `node --version` 退出码为 0）。
///
/// 与 `node_supports_module` 组合即可区分对拍三态：
/// 1. `!node_available()` → node 缺失（调用方应可见跳过）；
/// 2. `node_available() && !node_supports_module(m)` → node 在但缺能力（可见跳过）；
/// 3. `node_available() && node_supports_module(m)` → 对拍成立，此后任何失败必须 panic。
pub fn node_available() -> bool {
    Command::new(node_bin())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 当前 `node` 的版本字符串（如 `v22.3.0`）；不可用或输出为空时返回 `None`。
///
/// 仅用于 SKIP 诊断信息，不参与对拍判定。
pub fn node_version() -> Option<String> {
    let out = Command::new(node_bin()).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// 探测当前 `node` 能否 require 指定模块（如 `node:sqlite`）。
///
/// 以 `node -e "require.resolve('MODULE')"` 的退出码为准：0 = 模块存在且可 require；
/// 非 0（未知内置模块 / 文件缺失 / node 本身不存在）= 不支持。
pub fn node_supports_module(module: &str) -> bool {
    Command::new(node_bin())
        .arg("-e")
        .arg(format!("require.resolve({module:?})"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 已知模块的最低 Node 版本（只登记本仓库对拍用到的能力；未知模块返回 `None`）。
fn module_min_node_version(module: &str) -> Option<&'static str> {
    match module {
        // node:sqlite 自 v22.5.0 引入（早期版本还需 --experimental-sqlite 标志）。
        "node:sqlite" => Some("22.5.0"),
        _ => None,
    }
}

/// 一次 Node 子进程运行的完整结果（保留退出状态与 stderr 原文，支撑三态判定）。
pub struct NodeOutcome {
    /// 进程是否成功 spawn（`false` = node 可执行文件不存在 / 无法启动）。
    pub spawned: bool,
    /// 退出码是否为 0。
    pub ok: bool,
    /// stdout（trim 后，`\r\n` 归一为 `\n`）。
    pub stdout: String,
    /// stderr 原文（不裁剪，供失败信息透出）。
    pub stderr: String,
}

/// 运行 Node.js 并返回完整结果（含退出码与 stderr 原文；带超时防护）。
///
/// 与兼容包装 `node_run` 不同：这里不会把"退出码非 0"折叠成 `None` 而丢失证据。
pub fn node_run_outcome(js: &Path) -> NodeOutcome {
    let child = Command::new(node_bin())
        .arg(js)
        .current_dir(js.parent().unwrap_or(js))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return NodeOutcome {
                spawned: false,
                ok: false,
                stdout: String::new(),
                stderr: format!("node 进程无法启动: {e}"),
            };
        }
    };
    let out = finish_with_timeout(child);
    NodeOutcome {
        spawned: true,
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout)
            .trim()
            .replace("\r\n", "\n")
            .to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// 兼容旧签名：node 无法启动或退出码非 0 时返回 `None`（语义与改动前一致）。
///
/// **仅存量调用点保留**；新增对拍请用 `assert_e2e_matches_node_with_module`，
/// 否则又会退化成"node 侧失败 → 静默跳过"。
pub fn node_run(js: &Path) -> Option<String> {
    let outcome = node_run_outcome(js);
    (outcome.spawned && outcome.ok).then_some(outcome.stdout)
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
///
/// 等价于 `assert_e2e_matches_node_with_module(work, entry, None)`（不额外要求能力）。
pub fn assert_e2e_matches_node(work: &Path, entry: &str) -> String {
    assert_e2e_matches_node_with_module(work, entry, None)
}

/// 三态严格对拍：先跑 aluka 原生全链路，再按 node 侧状态决定"可见跳过"还是"必须一致"。
///
/// - node 缺失/无法启动 → 打印 `[SKIP node-e2e]` 标记（含原因）并返回本地输出；
/// - `required_module` 已指定但本机 node 缺该能力 → 打印 `[SKIP node-e2e]` 标记
///   （点名缺失模块与最低版本要求）并返回本地输出；
/// - node 可用且能力具备 → 严格对拍：退出码非 0 直接 panic（附 stderr 原文），
///   退出码 0 但输出与 aluka 不一致同样 panic。
///
/// 跳过路径一律走 `eprintln!`（`cargo test -- --nocapture` 可见），绝不静默。
pub fn assert_e2e_matches_node_with_module(
    work: &Path,
    entry: &str,
    required_module: Option<&str>,
) -> String {
    let rust_out = rust_pipeline_run(work, entry);

    if !node_available() {
        eprintln!(
            "[SKIP node-e2e] {entry}: 未执行 Node 对拍（此处无对拍证据）——本机无可用 node 可执行文件（NODE={}）；本用例仅验证 aluka 自身输出",
            node_bin()
        );
        return rust_out;
    }

    let missing_module = required_module.filter(|m| !node_supports_module(m));
    if let Some(module) = missing_module {
        let version = node_version().unwrap_or_else(|| "版本未知".to_string());
        let min = match module_min_node_version(module) {
            Some(v) => format!("需 Node ≥ {v}"),
            None => "需包含该模块的较新 Node 版本".to_string(),
        };
        eprintln!(
            "[SKIP node-e2e] {entry}: 未执行 Node 对拍（此处无对拍证据）——本机 node {version} 缺少用例所需模块 `{module}`（{min}）；本用例仅验证 aluka 自身输出"
        );
        return rust_out;
    }

    let outcome = node_run_outcome(&work.join(entry));
    if !outcome.spawned {
        eprintln!(
            "[SKIP node-e2e] {entry}: 未执行 Node 对拍（此处无对拍证据）——node 进程无法启动：{}；本用例仅验证 aluka 自身输出",
            outcome.stderr.trim()
        );
        return rust_out;
    }
    assert!(
        outcome.ok,
        "e2e Node 侧执行失败（{entry}）：node 退出码非 0，对拍未成立——这是真失败而非跳过。node stderr 原文:\n{}",
        outcome.stderr
    );
    assert_eq!(
        rust_out.trim(),
        outcome.stdout.trim(),
        "e2e 输出与 Node.js 22 不一致（{entry}）"
    );
    rust_out
}

/// 兼容别名
pub fn assert_e2e_matches_go(work: &Path, entry: &str) -> String {
    rust_pipeline_run(work, entry)
}
