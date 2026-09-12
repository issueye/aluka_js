//! test262 子集 runner（验证 JavaScript 标准语义 conformance）。
//!
//! 语料：`tests/conformance/test262/cases/*.js`，test262 风格
//! frontmatter（`/*--- ... ---*/`，支持 `negative: phase/type`）。执行前
//! 剥离 frontmatter、前置最小 assert harness，再由 `alukac` 编译 → `aluvm`
//! 执行。判定语义如下：
//! - 无 negative：正常退出（rc==0）即通过；
//! - `negative: phase: parse`：编译/运行输出含 SyntaxError 即通过；
//! - `negative: phase: runtime`：非零退出且输出含对应错误类型名。
//!
//! 语料目录缺失时整个测试跳过。`ALUKA_T262_FILTER=<子串>` 可单跑匹配用例。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CASE_WAIT: Duration = Duration::from_secs(30);

/// test262 风格断言 harness（逐字对齐 Go 版 run.go 的 `harness` 常量）。
const HARNESS: &str = r#"
var $DONOTEVALUATE = function() { throw new Error("$DONOTEVALUATE"); };
var assert = {
  sameValue: function(actual, expected, msg) {
    if (actual !== expected) {
      throw new Error("assert.sameValue: expected " + expected + " got " + actual + (msg ? " (" + msg + ")" : ""));
    }
  },
  isTrue: function(v, msg) { if (v !== true) throw new Error("assert.isTrue" + (msg ? ": " + msg : "")); },
  isFalse: function(v, msg) { if (v !== false) throw new Error("assert.isFalse" + (msg ? ": " + msg : "")); },
  notSameValue: function(actual, expected, msg) {
    if (actual === expected) {
      throw new Error("assert.notSameValue: expected not to be " + expected + (msg ? " (" + msg + ")" : ""));
    }
  },
  sameType: function(a, b, msg) {
    if (typeof a !== typeof b) throw new Error("assert.sameType: " + typeof a + " vs " + typeof b + (msg ? ": " + msg : ""));
  },
  throws: function(expectedType, fn) {
    var thrown = false;
    try { fn(); } catch (e) { thrown = true; }
    if (!thrown) throw new Error("assert.throws: no exception thrown");
  }
};
"#;

struct Negative {
    phase: String,
    err_type: String,
}

/// 解析 frontmatter `/*--- ... ---*/` 的 negative 语义（对齐 run.go）。
fn parse_negative(code: &str) -> Option<Negative> {
    let start = code.find("/*---")?;
    let body = &code[start + 5..];
    let end = body.find("---*/")?;
    let body = &body[..end];
    if !body.contains("negative") {
        return None;
    }
    let phase = find_kw(body, "phase").unwrap_or_else(|| "runtime".to_owned());
    let err_type = find_kw(body, "type").unwrap_or_else(|| "Error".to_owned());
    Some(Negative { phase, err_type })
}

/// `key: value` 行级扫描（Go 版用 `(?m)key:\s*(\w+)`）。
fn find_kw(body: &str, key: &str) -> Option<String> {
    let pat = format!("{key}:");
    for line in body.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix(&pat) {
            let val = rest.trim();
            let val = val
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '.')
                .next()?;
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }
    None
}

/// 剥离 frontmatter 块（对齐 run.go 的 stripFrontmatter）。
fn strip_frontmatter(code: &str) -> String {
    match (code.find("/*---"), code.find("---*/")) {
        (Some(s), Some(e)) => format!("{}{}", &code[..s], &code[e + 5..]),
        _ => code.to_owned(),
    }
}

/// 带超时运行命令（stdout+stderr 合并，对齐 run.go 的 CombinedOutput）。
///
/// 等待方式是「读线程阻塞在管道 EOF + 主线程 `recv_timeout`」，不再用
/// `try_wait` + `sleep(25ms)` 轮询：Windows 定时器粒度 15.6ms，实测 `sleep(25ms)`
/// 真实睡眠 ≈30ms，而 alukac/aluvm 单次分别只需约 5ms / 19ms——每个子进程都要
/// 白等一个定时器周期；本套件 154 例 × 最多 2 个子进程，这份白等原样计入门禁墙钟。
/// PATH 上查找可执行（node oracle 发现；找不到返回 None）
fn which_node(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand.to_string_lossy().into_owned());
        }
        #[cfg(windows)]
        if dir.join(format!("{name}.exe")).is_file() {
            return Some(
                dir.join(format!("{name}.exe"))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

fn run_with_timeout(cmd: &mut Command, wait: Duration) -> (Option<i32>, Vec<u8>, bool) {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("命令可执行");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
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

    let deadline = Instant::now() + wait;
    let mut timed_out = false;
    let mut output = Vec::new();
    // 按 stdout → stderr 顺序收（与 `>out 2>&1` 拼接口径一致）
    for rx in [&out_rx, &err_rx] {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => output.extend_from_slice(&chunk),
            // 超时：读线程会随子进程被杀、管道关闭而自行退出（不再 join）
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
    (code, output, timed_out)
}

/// 并发度：默认 `min(可用核数, 8)`；`ALUKA_T262_JOBS=1` 强制全串行。
///
/// 用例只写自己那份 `{用例名}.js` / `{用例名}.bc`（名字由用例名唯一推导，互不
/// 碰撞），且 154 例全是纯语义用例（无端口 / 无共享文件 / 无 worker / 无外呼），
/// 因此可安全并行。
fn t262_jobs() -> usize {
    match std::env::var("ALUKA_T262_JOBS")
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

/// 单个用例的判定结果（纯数据，供并行后按序汇总）。
struct CaseResult {
    /// 用例文件名
    name: String,
    /// 是否通过
    ok: bool,
    /// 失败原因（通过时为空串）
    reason: String,
    /// aluvm 退出码（未执行时为 None）
    code: Option<i32>,
    /// node 侧有效性：oracle 无法建立预期（正向 node 失败 / 负向 node
    /// 通过）时不计入通过或失败——对齐 node22 conformance 的 M1 防假阳性口径
    invalid: bool,
}

/// 跑单个用例：写临时用例 → alukac 编译 → aluvm 执行 → 判定。
///
/// 纯函数式（不打印、不累加计数），以便并行执行后按序汇总输出；临时产物名由
/// 用例名唯一推导，故多线程并行不冲突。
fn run_case(case: &Path, tmp: &Path, alukac: &str, aluvm: &str, node: Option<&str>) -> CaseResult {
    let name = case
        .file_name()
        .expect("有文件名")
        .to_string_lossy()
        .into_owned();
    let src = std::fs::read_to_string(case).expect("读用例");
    let negative = parse_negative(&src);
    // harness + 剥离 frontmatter 的用例体
    let js = tmp.join(format!("{name}.js"));
    std::fs::write(&js, format!("{HARNESS}\n{}", strip_frontmatter(&src))).expect("写临时用例");

    // node 侧 oracle 校验（M1 防假阳性口径）：正向用例 node 必须 rc=0，
    // 负向用例 node 必须非 0——node 与用例预期相悖时判 INVALID（不计入
    // 通过或失败；Sputnik 老用例含 getClass 等现实引擎皆无的 API）
    if let Some(node) = node {
        let mut node_cmd = Command::new(node);
        node_cmd.arg(&js).current_dir(tmp);
        let (node_code, _node_out, node_timeout) = run_with_timeout(&mut node_cmd, CASE_WAIT);
        let node_ok = if node_timeout {
            None
        } else {
            Some(node_code.is_some_and(|c| if negative.is_some() { c != 0 } else { c == 0 }))
        };
        if node_ok == Some(false) {
            return CaseResult {
                name,
                ok: false,
                reason: "INVALID：node 侧与用例预期相悖".to_owned(),
                code: node_code,
                invalid: true,
            };
        }
    }

    // 编译（parse 负例允许编译失败——错误输出参与判定）
    let bc = tmp.join(format!("{name}.bc"));
    let compiled = Command::new(alukac)
        .args(["compile", &js.to_string_lossy(), "-o"])
        .arg(&bc)
        .output()
        .expect("alukac 可执行");
    if !compiled.status.success() {
        // 编译失败：仅 parse 负例可凭 SyntaxError 判过
        let (ok, reason) = eval_result(
            negative.as_ref(),
            None,
            b"",
            Some(compiled.stderr.as_slice()),
        );
        return CaseResult {
            name,
            ok,
            reason,
            code: None,
            invalid: false,
        };
    }
    let mut vm_cmd = Command::new(aluvm);
    vm_cmd.arg("run").arg(&bc);
    let (vm_code, vm_out, timed_out) = run_with_timeout(&mut vm_cmd, CASE_WAIT);
    if timed_out {
        return CaseResult {
            name,
            ok: false,
            reason: format!("aluvm 超时（{CASE_WAIT:?}），疑似事件循环挂死"),
            code: vm_code,
            invalid: false,
        };
    }
    let (ok, reason) = eval_result(negative.as_ref(), vm_code, &vm_out, None);
    CaseResult {
        name,
        ok,
        reason,
        code: vm_code,
        invalid: false,
    }
}

/// 按 `jobs` 并发执行用例，返回**与入参同序**的结果（无锁取号 + 按序回填）。
fn run_cases_ordered(
    files: &[PathBuf],
    tmp: &Path,
    alukac: &str,
    aluvm: &str,
    node: Option<&str>,
    jobs: usize,
) -> Vec<CaseResult> {
    let jobs = jobs.clamp(1, files.len().max(1));
    if jobs <= 1 {
        return files
            .iter()
            .map(|case| run_case(case, tmp, alukac, aluvm, node))
            .collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<CaseResult>>> = (0..files.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(case) = files.get(i) else {
                        break;
                    };
                    let result = run_case(case, tmp, alukac, aluvm, node);
                    *slots[i].lock().expect("槽位锁未中毒") = Some(result);
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

/// 判定（对齐 run.go 的 evalResult）：
/// 返回（是否通过，失败原因）。`compile_err` 为 alukac 编译失败时的输出。
fn eval_result(
    negative: Option<&Negative>,
    vm_code: Option<i32>,
    output: &[u8],
    compile_err: Option<&[u8]>,
) -> (bool, String) {
    let low = |b: &[u8]| String::from_utf8_lossy(b).to_lowercase();
    let Some(neg) = negative else {
        if vm_code == Some(0) {
            return (true, String::new());
        }
        return (
            false,
            format!(
                "正向用例失败 vm_rc={vm_code:?}: {}",
                String::from_utf8_lossy(output)
            ),
        );
    };
    if neg.phase == "parse" {
        // 编译期报 SyntaxError（alukac stderr）或运行期输出均可
        let hit = compile_err
            .map(|e| {
                let l = low(e);
                l.contains("syntax error") || l.contains("syntaxerror")
            })
            .unwrap_or(false)
            || {
                let l = low(output);
                l.contains("syntax error") || l.contains("syntaxerror")
            };
        return if hit {
            (true, String::new())
        } else {
            (
                false,
                format!(
                    "parse 负例未报 SyntaxError（compile_err={:?}, out={:?}）",
                    compile_err.map(|e| String::from_utf8_lossy(e).to_string()),
                    String::from_utf8_lossy(output)
                ),
            )
        };
    }
    // runtime 负例：非零退出 + 输出含错误类型名（含带空格容错，逐字对齐 Go）
    if vm_code.is_none() || vm_code == Some(0) {
        return (
            false,
            format!(
                "runtime 负例未抛错（vm_rc={vm_code:?}）: {}",
                String::from_utf8_lossy(output)
            ),
        );
    }
    let l = low(output);
    let t = neg.err_type.to_lowercase();
    let spaced = t.replace("error", " error");
    if l.contains(&t) || (t.ends_with("error") && l.contains(&spaced)) {
        (true, String::new())
    } else {
        (
            false,
            format!(
                "runtime 负例类型不匹配（期望 {t}）: {}",
                String::from_utf8_lossy(output)
            ),
        )
    }
}

#[test]
fn test262_subset_conformance() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let case_dir = manifest_dir
        .ancestors()
        .nth(2)
        .map(|r| r.join("tests/conformance/test262/cases"))
        .filter(|d| d.is_dir());
    let Some(case_dir) = case_dir else {
        eprintln!("SKIP: 未找到 test262 子集语料（tests/conformance/test262/cases）");
        return;
    };
    let alukac = env!("CARGO_BIN_EXE_alukac");
    let aluvm = env!("CARGO_BIN_EXE_aluvm");
    // node oracle（M1 防假阳性校验用；缺席时跳过校验，全量仍跑）
    let node = std::env::var("NODE")
        .ok()
        .unwrap_or_else(|| "node".to_owned());
    let node = which_node(&node);
    let filter = std::env::var("ALUKA_T262_FILTER").ok();

    let tmp = std::env::temp_dir().join(format!("aluka_t262_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("创建临时目录");

    let mut files: Vec<PathBuf> = std::fs::read_dir(&case_dir)
        .expect("读语料目录")
        .map(|e| e.expect("读目录项").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("js"))
        .collect();
    files.sort();

    // 过滤子集（ALUKA_T262_FILTER），再按 `jobs` 并发执行后按序汇总
    let targets: Vec<PathBuf> = files
        .into_iter()
        .filter(|case| {
            filter.as_ref().is_none_or(|f| {
                case.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(f.as_str()))
            })
        })
        .collect();
    let jobs = t262_jobs();
    eprintln!("[t262] 共 {} 例（jobs={jobs}）", targets.len());
    let results = run_cases_ordered(&targets, &tmp, alukac, aluvm, node.as_deref(), jobs);

    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut invalid = 0usize;
    for r in results {
        if r.ok {
            eprintln!("PASS {}", r.name);
            pass += 1;
        } else if r.invalid {
            eprintln!("INV  {} (vm_rc={:?}) {}", r.name, r.code, r.reason);
            invalid += 1;
        } else {
            eprintln!("FAIL {} (vm_rc={:?}) {}", r.name, r.code, r.reason);
            failures.push(format!("{}: {}", r.name, r.reason));
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("----------------------------------------");
    eprintln!(
        "test262 subset: {pass}/{} passed（{} invalid）",
        pass + failures.len() + invalid,
        invalid
    );
    // M7.2 双层门禁：
    // - 手写回归语料（非 m72- 前缀）：**硬性全过**——任何失败即回归；
    // - 官方 test262 导入语料（m72- 前缀，tools_m72_import.py 生成）：
    //   基线推进期只断言下限（当前 800/1000），随引擎修复逐步上调至 100%
    //   （M7.2 验收口径），失败清单照常打印供分桶定位。
    let hand_failures: Vec<String> = failures
        .iter()
        .filter(|f| !f.starts_with("m72-"))
        .cloned()
        .collect();
    assert!(
        hand_failures.is_empty(),
        "test262 手写回归语料有 {} 例失败:\n{}",
        hand_failures.len(),
        hand_failures.join("\n")
    );
    let m72_failures = failures.len() - hand_failures.len();
    const M72_FLOOR: usize = 800;
    assert!(
        m72_failures <= 1000 - M72_FLOOR,
        "test262 官方导入语料通过数低于基线下限 {M72_FLOOR}/1000（当前失败 {m72_failures}）"
    );
}
