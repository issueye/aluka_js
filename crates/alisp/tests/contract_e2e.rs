//! M4 验收测试：玩具 Lisp 前端（alisp）→ `.aluc` → aluvm 执行。
//!
//! 契约证明：alisp 只依赖 aluka-bytecode 公共 API（见 crates/alisp/src/lib.rs
//! 的 use 面），产出符合 `docs/isa-aluc-spec.md` 的容器，aluvm 魔数嗅探直接
//! 执行——新语法前端零改后端接入。

use std::path::{Path, PathBuf};
use std::process::Command;

fn alisp_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_alisp")).to_path_buf()
}

/// aluvm 二进制定位：优先 ALUVM_EXE 环境变量，回退工作区 target 目录。
/// 找不到时返回 None（调用方跳过执行段——`cargo test -p alisp` 单包构建
/// 不一定产出 aluvm；全工作区 `cargo test` 必然存在）。
fn aluvm_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ALUVM_EXE") {
        return Some(PathBuf::from(p));
    }
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/aluvm.exe");
    candidate.is_file().then_some(candidate)
}

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alisp_e2e_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录");
    dir
}

/// 验收核心：Lisp 程序（函数定义/递归/条件/算术/字符串/print）编译为
/// `.aluc` 后在 aluvm 上输出正确结果——全程零改后端。
#[test]
fn lisp_program_runs_on_aluvm() {
    let work = work_dir("program");
    let src = concat!(
        "; M4 验收：函数定义/递归/条件/算术/字符串\n",
        "(defn add (a b) (+ a b))\n",
        "(defn fact (n) (if (< n 2) 1 (* n (fact (- n 1)))))\n",
        "(print (add 2 3))\n",
        "(print (fact 5))\n",
        "(print \"hello lisp\")\n",
        "(print (* 6 7))\n",
    );
    std::fs::write(work.join("demo.lisp"), src).unwrap();

    let bc = work.join("demo.aluc");
    let out = Command::new(alisp_exe())
        .arg(work.join("demo.lisp"))
        .arg("-o")
        .arg(&bc)
        .output()
        .expect("alisp 运行");
    assert!(out.status.success(), "alisp 编译失败");

    let Some(aluvm) = aluvm_exe() else {
        eprintln!("跳过执行段：aluvm 不存在（单包测试构建）");
        return;
    };
    let run = Command::new(&aluvm)
        .arg("run")
        .arg(&bc)
        .current_dir(&work)
        .output()
        .expect("aluvm 运行");
    assert!(run.status.success(), "aluvm 执行失败");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        stdout.trim(),
        "5\n120\nhello lisp\n42",
        "递归/条件/算术/输出全部经 ISA 契约执行"
    );
}

/// 容器格式：alisp 产物以 ALUKACC1 魔数开头（发布容器）。
#[test]
fn alisp_emits_aluc_container() {
    let work = work_dir("container");
    std::fs::write(work.join("m.lisp"), "(print 1)\n").unwrap();
    let bc = work.join("m.aluc");
    let out = Command::new(alisp_exe())
        .arg(work.join("m.lisp"))
        .arg("-o")
        .arg(&bc)
        .output()
        .expect("alisp 运行");
    assert!(out.status.success());
    let bytes = std::fs::read(&bc).unwrap();
    assert_eq!(&bytes[0..8], b"ALUKACC1", "产物必须是 ALUKACC1 容器");
}

/// `.alua` 文本汇编：确定性转储且包含函数/指令/常量节。
#[test]
fn alua_dump_is_deterministic() {
    let work = work_dir("alua");
    std::fs::write(work.join("m.lisp"), "(print (* 6 7))\n").unwrap();
    let dump = work.join("m.alua");
    let out = Command::new(alisp_exe())
        .arg(work.join("m.lisp"))
        .arg("--disasm")
        .output()
        .expect("alisp 运行");
    assert!(out.status.success());
    let text = std::fs::read_to_string(&dump).unwrap();
    let text2 = {
        let _ = std::fs::remove_file(&dump);
        let _ = Command::new(alisp_exe())
            .arg(work.join("m.lisp"))
            .arg("--disasm")
            .output()
            .unwrap();
        std::fs::read_to_string(&dump).unwrap()
    };
    assert_eq!(text, text2, "转储必须确定性");
    assert!(text.contains(".func main"));
    assert!(text.contains("CALL_METHOD"));
}
