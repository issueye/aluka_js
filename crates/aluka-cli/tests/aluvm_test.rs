//! aluvm CLI 集成测试：版本、黄金语料执行、process.argv 注入、未捕获异常退出码。

use aluka_bytecode::{BytecodeModule, Constant, FuncTemplate, Instr, Op};
use std::process::Command;

fn aluvm_exe() -> &'static str {
    env!("CARGO_BIN_EXE_aluvm")
}

fn corpus_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/golden/corpus")
        .join(name)
}

/// 构造最小字节码模块并序列化到临时文件，返回文件路径。
fn write_module(
    dir: &std::path::Path,
    name: &str,
    constants: Vec<Constant>,
    code: Vec<Instr>,
) -> std::path::PathBuf {
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: 0,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code,
            max_stack: 8,
            source_file: name.to_owned(),
            constants,
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        }],
        classes: Vec::new(),
    };
    let data = module.serialize();
    let path = dir.join(name);
    std::fs::write(&path, &data).expect("写临时 .bc 失败");
    // 往返自检：反序列化 + Verifier 必须通过
    let round_trip = BytecodeModule::deserialize(&data).expect("往返反序列化失败");
    round_trip.verify().expect("往返模块未通过 Verifier");
    path
}

#[test]
fn aluvm_version_exits_successfully() {
    let out = Command::new(aluvm_exe())
        .arg("--version")
        .output()
        .expect("运行 aluvm 失败");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("aluvm"), "版本输出应包含 aluvm: {stdout}");
}

#[test]
fn aluvm_runs_golden_corpus_and_matches_node() {
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(corpus_path("01_arithmetic_bitwise.bc"))
        .output()
        .expect("运行 aluvm 失败");
    assert!(out.status.success(), "执行黄金语料应成功");
    let rust_out = String::from_utf8_lossy(&out.stdout);
    assert_eq!(rust_out.trim(), "20 36 23 46 23 -24 24 24");

    // 运行 Node.js 22 LTS 对拍
    let src = corpus_path("01_arithmetic_bitwise.bc")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("sources/01_arithmetic_bitwise.js");
    let node_bin = std::env::var("NODE").unwrap_or_else(|_| "node".to_string());
    if let Ok(node_out) = Command::new(node_bin).arg(&src).output() {
        if node_out.status.success() {
            assert_eq!(
                rust_out.trim(),
                String::from_utf8_lossy(&node_out.stdout).trim(),
                "aluvm CLI 输出必须与 Node.js 22 一致"
            );
        }
    }
}

#[test]
fn aluvm_reports_uncaught_exception_with_failure_exit() {
    let dir = std::env::temp_dir().join("aluvm_test_uncaught");
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let bc = write_module(
        &dir,
        "uncaught_throw.bc",
        vec![Constant::String("str-err".to_owned())],
        vec![Instr::new(Op::PushConst, 0), Instr::new(Op::Throw, 0)],
    );
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(&bc)
        .output()
        .expect("运行 aluvm 失败");
    assert!(!out.status.success(), "未捕获异常必须以非零退出码结束");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("str-err"), "stderr 应包含异常值: {stderr}");
}

#[test]
fn aluvm_injects_process_argv() {
    let dir = std::env::temp_dir().join("aluvm_test_argv");
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    // console.log(process.argv[1])
    let bc = write_module(
        &dir,
        "argv_echo.bc",
        vec![
            Constant::String("console".to_owned()),
            Constant::String("process".to_owned()),
            Constant::String("argv".to_owned()),
            Constant::String("log".to_owned()),
        ],
        vec![
            Instr::new(Op::LoadGlobal, 0),
            Instr::new(Op::LoadGlobal, 1),
            Instr::new(Op::GetProp, 2),
            Instr::new(Op::PushInt, 1),
            Instr::new(Op::GetElem, 0),
            Instr::new(Op::CallMethod, (1 << 16) | 3),
            Instr::new(Op::Pop, 0),
            Instr::new(Op::ReturnUndef, 0),
        ],
    );
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(&bc)
        .arg("hello-argv")
        .output()
        .expect("运行 aluvm 失败");
    assert!(
        out.status.success(),
        "argv 注入用例应成功: {:?}",
        out.stderr
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "hello-argv",
        "process.argv[1] 应为第一个 CLI 参数"
    );
}
