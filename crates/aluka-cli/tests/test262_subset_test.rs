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
use std::path::PathBuf;
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
fn run_with_timeout(cmd: &mut Command, wait: Duration) -> (Option<i32>, Vec<u8>, bool) {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("命令可执行");
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
    (code, output, timed_out)
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

    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for case in &files {
        let name = case
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();
        if let Some(f) = &filter {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        let src = std::fs::read_to_string(case).expect("读用例");
        let negative = parse_negative(&src);
        // harness + 剥离 frontmatter 的用例体
        let js = tmp.join(format!("{name}.js"));
        std::fs::write(&js, format!("{HARNESS}\n{}", strip_frontmatter(&src))).expect("写临时用例");

        // 编译（parse 负例允许编译失败——错误输出参与判定）
        let bc = tmp.join(format!("{name}.bc"));
        let compiled = Command::new(alukac)
            .args(["compile", &js.to_string_lossy(), "-o"])
            .arg(&bc)
            .output()
            .expect("alukac 可执行");
        let compile_err = (!compiled.status.success()).then_some(compiled.stderr.as_slice());
        let (ok, reason, code);
        if compiled.status.success() {
            let mut vm_cmd = Command::new(aluvm);
            vm_cmd.arg("run").arg(&bc);
            let (vm_code, vm_out, timed_out) = run_with_timeout(&mut vm_cmd, CASE_WAIT);
            if timed_out {
                (ok, reason, code) = (
                    false,
                    format!("aluvm 超时（{CASE_WAIT:?}），疑似事件循环挂死"),
                    vm_code,
                );
            } else {
                let (verdict, why) = eval_result(negative.as_ref(), vm_code, &vm_out, None);
                (ok, reason, code) = (verdict, why, vm_code);
            }
        } else {
            // 编译失败：仅 parse 负例可凭 SyntaxError 判过
            let (verdict, why) = eval_result(negative.as_ref(), None, b"", compile_err);
            (ok, reason, code) = (verdict, why, None);
        }
        if ok {
            eprintln!("PASS {name}");
            pass += 1;
        } else {
            eprintln!("FAIL {name} (vm_rc={code:?}) {reason}");
            failures.push(format!("{name}: {reason}"));
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("----------------------------------------");
    eprintln!("test262 subset: {pass}/{} passed", pass + failures.len());
    assert!(
        failures.is_empty(),
        "test262 子集有 {} 例失败:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
