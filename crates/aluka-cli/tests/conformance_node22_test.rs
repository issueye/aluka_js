//! node22 conformance 集成测试（固化自 `.work/scratch/m2_conf/run_rust_conf.sh`）：
//! `alukac` 编译用例 → `aluvm` 执行 → stdout 与 `node` 逐字节对拍。
//!
//! 口径与脚本一致：
//! - `.mjs` 依赖先编译到与主用例 `.bc` 相同的临时目录（`require` 相对解析
//!   基于 `base_dir` = 主 `.bc` 所在目录）；
//! - node / aluvm 都在语料目录下运行（相对 `require` 与 pem 资源可用），
//!   stderr 并入对比流（复刻脚本 `>out 2>&1`）；
//! - node 侧自身失败（rc ∉ {0, 124}）→ 视为无效对比，跳过不计失败
//!   （M1 防假阳性口径）；
//! - 与脚本的两处偏差：主用例编译失败记**失败**（脚本 SKIP 会掩盖编译
//!   回归）；aluvm 超时（30s，防事件循环挂死拖垮整个测试）记**失败**。
//!
//! 语料在工作区 `tests/conformance/node22/cases/`。
//! **执行前把语料复制到临时目录**再跑 node/aluvm：node 侧用例（如
//! trace-events）会向 cwd 写运行产物（`node_trace.1.log`），不能落回
//! 语料目录。语料目录或 `node` 不存在时整个测试跳过（CI 无 node
//! 环境不算失败）。`ALUKA_CONF_FILTER=<子串>` 可只跑名字匹配的用例。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 单用例超时（node 与 aluvm 各自计；脚本为 20s，放宽到 30s）。
const CASE_WAIT: Duration = Duration::from_secs(30);

struct RunOutcome {
    code: Option<i32>,
    output: Vec<u8>,
    timed_out: bool,
}

/// 带超时地运行命令（stdout 与 stderr 合并收集，复刻 `>out 2>&1`）。
fn run_with_timeout(cmd: &mut Command, wait: Duration) -> RunOutcome {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunOutcome {
                code: None,
                output: format!("<spawn 失败: {e}>").into_bytes(),
                timed_out: false,
            };
        }
    };
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if started.elapsed() > wait {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };
    let timed_out = code.is_none() && started.elapsed() > wait;
    let mut output = out_reader.join().unwrap_or_default();
    output.extend_from_slice(&err_reader.join().unwrap_or_default());
    RunOutcome {
        code,
        output,
        timed_out,
    }
}

fn head_lines(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let head: Vec<&str> = text.lines().take(3).collect();
    head.join(" | ")
}

#[test]
fn node22_conformance_matches_node_stdout() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let case_dir = manifest_dir
        .ancestors()
        .nth(2)
        .map(|r| r.join("tests/conformance/node22/cases"))
        .filter(|d| d.is_dir());
    let Some(case_dir) = case_dir else {
        eprintln!("SKIP: 未找到 conformance 语料（tests/conformance/node22/cases）");
        return;
    };
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_owned());
    if Command::new(&node).arg("--version").output().is_err() {
        eprintln!("SKIP: node 不在 PATH（可用 NODE=<路径> 指定）");
        return;
    }
    let alukac = env!("CARGO_BIN_EXE_alukac");
    let aluvm = env!("CARGO_BIN_EXE_aluvm");
    let filter = std::env::var("ALUKA_CONF_FILTER").ok();

    let tmp = std::env::temp_dir().join(format!("aluka_conf_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("创建临时目录");
    // 语料工作副本：node/aluvm 在这里运行，运行产物不污染原始语料目录
    let work_dir = tmp.join("cases");
    std::fs::create_dir_all(&work_dir).expect("创建语料副本目录");
    for entry in std::fs::read_dir(&case_dir).expect("读语料目录") {
        let path = entry.expect("读目录项").path();
        if !path.is_file() {
            continue;
        }
        // 跳过历史运行产物与编译产物
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("log") | Some("bc")
        ) {
            continue;
        }
        let dest = work_dir.join(path.file_name().expect("有文件名"));
        std::fs::copy(&path, &dest).expect("复制语料文件");
    }

    // .mjs 依赖先编译（失败忽略，与脚本 `|| true` 一致——依赖缺失时主用例
    // 会在运行期报模块找不到，对拍同样能暴露差异）
    let mut deps = Vec::new();
    for entry in std::fs::read_dir(&case_dir).expect("读语料目录") {
        let entry = entry.expect("读目录项");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("mjs") {
            deps.push(path);
        }
    }
    deps.sort();
    for dep in &deps {
        let stem = dep
            .file_stem()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();
        let _ = Command::new(alukac)
            .args(["compile", &dep.to_string_lossy(), "-o"])
            .arg(tmp.join(format!("{stem}.bc")))
            .output();
    }

    // 主用例：*.cjs + *.mjs，字典序保证输出稳定
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&case_dir)
        .expect("读语料目录")
        .map(|e| e.expect("读目录项").path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("cjs") | Some("mjs")
            )
        })
        .collect();
    cases.sort();
    let cases: Vec<PathBuf> = cases
        .into_iter()
        .filter(|p| {
            filter
                .as_ref()
                .is_none_or(|f| p.to_string_lossy().contains(f.as_str()))
        })
        .collect();

    let mut pass = 0usize;
    let mut invalid = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let name = case
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();

        // 1. node 期望输出（stdout+stderr；cwd = 语料副本目录）
        let mut node_cmd = Command::new(&node);
        node_cmd.arg(&name).current_dir(&work_dir);
        let node_out = run_with_timeout(&mut node_cmd, CASE_WAIT);

        // 2. node 侧自身失败（rc ∉ {0,124}）→ 无效对比，不进入编译/执行
        //    （此类语料 node 都跑不起来，编译失败不构成 Rust 侧回归）
        if node_out.code.is_none_or(|c| c != 0 && c != 124) {
            eprintln!(
                "INV  {name} (node rc={:?}) —— node 侧自身失败，无效对比",
                node_out.code
            );
            invalid += 1;
            continue;
        }

        // 3. alukac 编译主用例（编译失败 = 回归，记失败而非跳过）
        let bc = tmp.join(format!("{name}.bc"));
        let compiled = Command::new(alukac)
            .args(["compile", &case.to_string_lossy(), "-o"])
            .arg(&bc)
            .output()
            .expect("alukac 可执行");
        if !compiled.status.success() {
            failures.push(format!(
                "{name}: alukac 编译失败: {}",
                String::from_utf8_lossy(&compiled.stderr).trim()
            ));
            continue;
        }

        // 4. aluvm 执行（cwd = 语料副本目录，支持相对 require/pem）
        let mut vm_cmd = Command::new(aluvm);
        vm_cmd.arg("run").arg(&bc).current_dir(&work_dir);
        let vm_out = run_with_timeout(&mut vm_cmd, CASE_WAIT);

        if vm_out.timed_out {
            failures.push(format!(
                "{name}: aluvm 超时（{CASE_WAIT:?}）——疑似事件循环挂死；vm 输出头: {}",
                head_lines(&vm_out.output)
            ));
            continue;
        }
        if node_out.output == vm_out.output {
            eprintln!("PASS {name}");
            pass += 1;
        } else {
            failures.push(format!(
                "{name}: stdout 不一致 (node_rc={:?} vm_rc={:?})\n  node: {}\n  vm  : {}",
                node_out.code,
                vm_out.code,
                head_lines(&node_out.output),
                head_lines(&vm_out.output)
            ));
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("----------------------------------------");
    eprintln!(
        "Result: {pass}/{} passed, {invalid} invalid",
        pass + failures.len()
    );
    assert!(
        failures.is_empty(),
        "conformance 有 {} 例失败:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
