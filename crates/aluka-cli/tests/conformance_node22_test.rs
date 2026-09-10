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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 单用例超时（node 与 aluvm 各自计；脚本为 20s，放宽到 30s）。
const CASE_WAIT: Duration = Duration::from_secs(30);
/// 单个用例的三态结果（顺序与 `cases` 一一对应，便于顺序输出）。
enum CaseOutcome {
    /// 通过（用例名）
    Pass(String),
    /// 无效对比（node 侧自身失败；用例名 + node 退出码）
    Invalid(String, Option<i32>),
    /// 失败（已格式化的失败说明）
    Fail(String),
}

/// 跑一个用例所需的固定上下文（全部只读，可被多个 worker 共享）。
struct RunCtx<'a> {
    node: &'a str,
    alukac: &'a str,
    aluvm: &'a str,
    /// node / aluvm 的工作目录（语料副本目录）
    work_dir: &'a std::path::Path,
    /// 编译产物与依赖 `.bc` 的临时目录
    tmp: &'a std::path::Path,
}

/// 跑单个用例：node 取期望输出 → alukac 编译 → aluvm 执行 → 逐字节对拍。
/// 纯函数式（不打印、不累加计数），以便并行执行后按序汇总输出。
fn run_case(ctx: &RunCtx<'_>, case: &std::path::Path, name: &str) -> CaseOutcome {
    // 1. node 期望输出（stdout+stderr；cwd = 语料副本目录）
    let mut node_cmd = Command::new(ctx.node);
    node_cmd.arg(name).current_dir(ctx.work_dir);
    let node_out = run_with_timeout(&mut node_cmd, CASE_WAIT);

    // 2. node 侧自身失败（rc ∉ {0,124}）→ 无效对比，不进入编译/执行
    //    （此类语料 node 都跑不起来，编译失败不构成 Rust 侧回归）
    if node_out.code.is_none_or(|c| c != 0 && c != 124) {
        return CaseOutcome::Invalid(name.to_owned(), node_out.code);
    }

    // 3. alukac 编译主用例（编译失败 = 回归，记失败而非跳过）
    //    产物名由用例名唯一推导（`gen/a.cjs` → `gen_a.cjs.bc`），并行时不冲突。
    let bc = ctx.tmp.join(format!("{}.bc", name.replace('/', "_")));
    let compiled = Command::new(ctx.alukac)
        .args(["compile", &case.to_string_lossy(), "-o"])
        .arg(&bc)
        .output()
        .expect("alukac 可执行");
    if !compiled.status.success() {
        return CaseOutcome::Fail(format!(
            "{name}: alukac 编译失败: {}",
            String::from_utf8_lossy(&compiled.stderr).trim()
        ));
    }

    // 4. aluvm 执行（cwd = 语料副本目录，支持相对 require/pem）
    let mut vm_cmd = Command::new(ctx.aluvm);
    vm_cmd.arg("run").arg(&bc).current_dir(ctx.work_dir);
    let vm_out = run_with_timeout(&mut vm_cmd, CASE_WAIT);

    if vm_out.timed_out {
        return CaseOutcome::Fail(format!(
            "{name}: aluvm 超时（{CASE_WAIT:?}）——疑似事件循环挂死；vm 输出头: {}",
            head_lines(&vm_out.output)
        ));
    }
    if node_out.output == vm_out.output {
        CaseOutcome::Pass(name.to_owned())
    } else {
        CaseOutcome::Fail(format!(
            "{name}: stdout 不一致 (node_rc={:?} vm_rc={:?})\n  node: {}\n  vm  : {}",
            node_out.code,
            vm_out.code,
            head_lines(&node_out.output),
            head_lines(&vm_out.output)
        ))
    }
}

/// 用例是否触碰**进程/机器级共享资源**：固定端口、cwd 固定文件名、真网外呼，
/// 或起子进程 / worker / cluster。
///
/// 这类用例必须串行——原设计「默认全串行」正是为此（语料会绑固定端口或向 cwd
/// 写固定文件名）。其余纯语义用例彼此完全独立（各自只写由用例名唯一推导的
/// `.bc`），可安全并行。判定**故意放宽**：宁可多判几个去串行，也不让共享资源
/// 用例落进并发桶；源码读不出来时按敏感处理。
fn is_isolation_sensitive(case: &Path) -> bool {
    const MARKERS: &[&str] = &[
        "listen(",
        "createServer",
        "createConnection",
        ".connect(",
        "net.",
        "dgram",
        "cluster",
        "worker_threads",
        "child_process",
        "spawnSync",
        "writeFile",
        "appendFile",
        "mkdir",
        "createWriteStream",
        "createReadStream",
        "fs.",
        "fetch(",
        "http",
        "tls",
        "dns",
        "process.env",
        ".pem",
    ];
    let Ok(src) = std::fs::read_to_string(case) else {
        return true;
    };
    MARKERS.iter().any(|m| src.contains(m))
}

/// 并发度：默认 `min(可用核数, 8)`（每例要起 3 个子进程，核数打满后收益递减）；
/// `ALUKA_CONF_JOBS=1` 可强制退回全串行（排查疑似并发干扰时用）。
fn conf_jobs() -> usize {
    match std::env::var("ALUKA_CONF_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) => n.max(1),
        None => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8),
    }
}

/// 执行全部用例，返回**与入参同序**的结果。
///
/// 每个用例要跑 3 个子进程（node / alukac / aluvm）。872 例逐例计时实测各阶段
/// 占比：node 66% / alukac 13% / aluvm 19%，分项之和占墙钟 99.0%（无空转、
/// 最慢用例仅 335 ms、0 例超时）——即耗时**几乎全是进程启动**，用例之间彼此
/// 独立，属典型可并行负载。单测拿不到 libtest 的并行（那只在测试函数之间），
/// 只能在这里自己开线程，否则 11/12 个逻辑核全程闲置。
///
/// `jobs` 为并发度，1 = 顺序执行。
fn run_cases(cases: &[(PathBuf, String)], ctx: &RunCtx<'_>, jobs: usize) -> Vec<CaseOutcome> {
    let jobs = jobs.clamp(1, cases.len().max(1));
    if jobs <= 1 {
        return cases
            .iter()
            .map(|(case, name)| run_case(ctx, case, name))
            .collect();
    }

    // 无锁取号 + 按序回填：结果顺序与 `cases` 一致，输出（PASS/INV/失败）
    // 因此与顺序执行逐字节相同。
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<CaseOutcome>>> = (0..cases.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((case, name)) = cases.get(i) else {
                        break;
                    };
                    let outcome = run_case(ctx, case, name);
                    *slots[i].lock().expect("槽位锁未中毒") = Some(outcome);
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("槽位锁未中毒")
                .expect("槽位已填充")
        })
        .collect()
}

struct RunOutcome {
    code: Option<i32>,
    output: Vec<u8>,
    timed_out: bool,
}

/// 带超时地运行命令（stdout 与 stderr 合并收集，复刻 `>out 2>&1`）。
///
/// **不用轮询**：`Child::try_wait` + `sleep(interval)` 的写法受 Windows 默认
/// 15.6 ms 定时器粒度所限——实测 `sleep(1ms)` 真实睡眠 ≈ 15 ms、`sleep(25ms)`
/// ≈ 30 ms，故「调小轮询间隔」并不能减少等待。这里让读取线程阻塞在管道 EOF
/// （子进程退出即关闭 stdout/stderr）上，主线程用 `recv_timeout` 精确等待，
/// 每个子进程省下约一个定时器周期的白等（本套件 3 个子进程 × 873 例）。
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
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    // 读取线程在管道 EOF 时立即返回并将缓冲送回；主线程按 stdout → stderr
    // 的顺序收（与 `>out 2>&1` 拼接口径一致）。
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut buf);
        let _ = out_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut buf);
        let _ = err_tx.send(buf);
    });

    let started = Instant::now();
    let deadline = started + wait;
    let mut timed_out = false;
    let mut output = Vec::new();
    for rx in [&out_rx, &err_rx] {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(buf) => output.extend_from_slice(&buf),
            // 超时：读取线程会在子进程被杀后随管道关闭自行退出（不再 join）
            Err(_) => timed_out = true,
        }
    }
    let code = if timed_out {
        let _ = child.kill();
        let _ = child.wait();
        None
    } else {
        // 管道已 EOF ≈ 子进程已退出，wait() 立即返回
        child.wait().ok().and_then(|status| status.code())
    };
    RunOutcome {
        code,
        output,
        timed_out: code.is_none() && started.elapsed() > wait,
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
        if path.is_dir() {
            // 子目录（如 gen/ 生成语料）：建同级目录并递归复制
            let sub = work_dir.join(path.file_name().expect("有文件名"));
            std::fs::create_dir_all(&sub).expect("创建语料子目录");
            for sub_entry in std::fs::read_dir(&path).expect("读语料子目录") {
                let sp = sub_entry.expect("读子目录项").path();
                if !sp.is_file() {
                    continue;
                }
                if matches!(
                    sp.extension().and_then(|e| e.to_str()),
                    Some("log") | Some("bc")
                ) {
                    continue;
                }
                let dest = sub.join(sp.file_name().expect("有文件名"));
                std::fs::copy(&sp, &dest).expect("复制语料子文件");
            }
            continue;
        }
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

    // 主用例：*.cjs + *.mjs，字典序保证输出稳定（含 gen/ 生成语料子目录）；
    // 元组第二项 = 相对语料目录的运行路径（子目录文件带目录前缀）
    let mut cases: Vec<(PathBuf, String)> = std::fs::read_dir(&case_dir)
        .expect("读语料目录")
        .map(|e| e.expect("读目录项").path())
        .flat_map(|p| {
            if p.is_dir() {
                // 子目录（如 gen/）：递归一层收集用例文件
                let dir_name = p
                    .file_name()
                    .expect("有目录名")
                    .to_string_lossy()
                    .into_owned();
                std::fs::read_dir(&p)
                    .expect("读语料子目录")
                    .map(|e| e.expect("读子目录项").path())
                    .map(|f| {
                        let rel = format!(
                            "{dir_name}/{}",
                            f.file_name().expect("有文件名").to_string_lossy()
                        );
                        (f, rel)
                    })
                    .collect::<Vec<_>>()
            } else {
                let rel = p
                    .file_name()
                    .expect("有文件名")
                    .to_string_lossy()
                    .into_owned();
                vec![(p, rel)]
            }
        })
        .filter(|(p, _)| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("cjs") | Some("mjs")
            )
        })
        .collect();
    cases.sort_by(|a, b| a.1.cmp(&b.1));
    let cases: Vec<(PathBuf, String)> = cases
        .into_iter()
        .filter(|(p, rel)| {
            filter.as_ref().is_none_or(|f| {
                p.to_string_lossy().contains(f.as_str()) || rel.contains(f.as_str())
            })
        })
        .collect();
    let ctx = RunCtx {
        node: &node,
        alukac,
        aluvm,
        work_dir: &work_dir,
        tmp: &tmp,
    };
    // 分区执行 + 按原始顺序回填：隔离敏感用例串行，其余按 `conf_jobs()` 并行。
    // 回填保证输出顺序与「全串行」逐字节一致（PASS/INV/失败的行序不变）。
    let mut sensitive: Vec<usize> = Vec::new();
    let mut pure: Vec<usize> = Vec::new();
    for (i, (case, _)) in cases.iter().enumerate() {
        if is_isolation_sensitive(case) {
            sensitive.push(i);
        } else {
            pure.push(i);
        }
    }
    let jobs = conf_jobs();
    eprintln!(
        "[conf] 共 {} 例：隔离敏感 {} 例串行 / 纯语义 {} 例并行（jobs={jobs}）",
        cases.len(),
        sensitive.len(),
        pure.len()
    );
    let pick = |idx: &[usize]| -> Vec<(PathBuf, String)> {
        idx.iter().map(|&i| cases[i].clone()).collect()
    };
    let mut slots: Vec<Option<CaseOutcome>> = (0..cases.len()).map(|_| None).collect();
    for (slot, outcome) in sensitive.iter().zip(run_cases(&pick(&sensitive), &ctx, 1)) {
        slots[*slot] = Some(outcome);
    }
    for (slot, outcome) in pure.iter().zip(run_cases(&pick(&pure), &ctx, jobs)) {
        slots[*slot] = Some(outcome);
    }
    let outcomes: Vec<CaseOutcome> = slots
        .into_iter()
        .map(|slot| slot.expect("每个用例都已由某一批次回填"))
        .collect();

    let mut pass = 0usize;
    let mut invalid = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for outcome in outcomes {
        match outcome {
            CaseOutcome::Pass(name) => {
                eprintln!("PASS {name}");
                pass += 1;
            }
            CaseOutcome::Invalid(name, code) => {
                eprintln!("INV  {name} (node rc={code:?}) —— node 侧自身失败，无效对比");
                invalid += 1;
            }
            CaseOutcome::Fail(msg) => failures.push(msg),
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
