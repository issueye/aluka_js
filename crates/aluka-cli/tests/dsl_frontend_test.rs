//! ISA 契约实证性 DSL 前端端到端集成测试套件。
//!
//! 验证目标：
//! 1. 验证 S 表达式函数式 DSL 前端（.adsl / .lisp）无缝接入 LanguageRegistry 与编译器；
//! 2. 编译产物完全符合统一 ISA 规范（ALUKABC1 Version 30）并通过 Verifier 严格校验；
//! 3. 产出的二进制字节码在 Rust VM (aluvm) 执行，实现控制台输出 100% 逐字符合 DSL 语义；
//! 4. 证明后端（VM/Runtime）无需做任何侵入性修改即可即插即用接入新语言前端。

use std::fs;
use std::process::Command;

use aluka_bytecode::BytecodeModule;
use aluka_vm::Vm;

#[test]
fn test_dsl_frontend_e2e_compilation_and_execution() {
    let alukac_bin = env!("CARGO_BIN_EXE_alukac");

    let temp_dir = std::env::temp_dir().join("aluka_dsl_oracle_test");
    let _ = fs::create_dir_all(&temp_dir);

    let dsl_file = temp_dir.join("pipeline_demo.adsl");
    let bc_file = temp_dir.join("pipeline_demo.bc");

    // 编写包含算术、条件分支、多参数函数、高阶函数和控制台打印的 DSL 源码
    let dsl_source = r#"
        ;; ============================================
        ;; 1. 基础算术与变量定义
        ;; ============================================
        (def x 12)
        (def y 8)
        (def sum (+ x y))
        (console.log "sum:" sum)

        ;; ============================================
        ;; 2. 条件分支计算
        ;; ============================================
        (def is_big (> sum 15))
        (if is_big
            (console.log "branch:" "greater than 15")
            (console.log "branch:" "less or equal 15"))

        ;; ============================================
        ;; 3. 多参数函数定义与调用
        ;; ============================================
        (fn poly (a b c)
            (+ (* a b) (/ c 2)))

        (def poly_val (poly 6 7 10))
        (console.log "poly result:" poly_val)

        ;; ============================================
        ;; 4. 一等公民函数与高阶函数应用 (Higher-Order Function)
        ;; ============================================
        (fn apply_twice (fn_val arg)
            (fn_val (fn_val arg)))

        (fn inc (n)
            (+ n 5))

        (def ho_result (apply_twice inc 100))
        (console.log "higher order result:" ho_result)
    "#;

    fs::write(&dsl_file, dsl_source).expect("写入 DSL 源码文件失败");

    // 1. 使用 alukac CLI 编译 .adsl 源码
    let compile_status = Command::new(alukac_bin)
        .arg("compile")
        .arg(&dsl_file)
        .arg("-o")
        .arg(&bc_file)
        .output()
        .expect("运行 alukac 编译器失败");

    assert!(
        compile_status.status.success(),
        "alukac 编译 DSL 失败: {}",
        String::from_utf8_lossy(&compile_status.stderr)
    );

    // 2. 静态规范校验
    let bc_bytes = fs::read(&bc_file).expect("读取编译产物 .bc 失败");
    let module = BytecodeModule::deserialize(&bc_bytes).expect("反序列化字节码模块失败");
    assert_eq!(module.version, 30, "字节码容器版本号必须为 30");
    module.verify().expect("DSL 编译产物未通过 ISA 静态校验");

    // 3. 在 Rust VM (aluvm) 中执行并捕获输出
    let mut rust_vm = Vm::new(0);
    rust_vm
        .run_module(&module)
        .expect("Rust VM 执行 DSL 字节码模块失败");
    let rust_output = rust_vm.stdout_records.join("\n").trim().to_string();

    // 4. 断言 Rust VM 输出符合预期
    let expected_lines = [
        "sum: 20",
        "branch: greater than 15",
        "poly result: 47",
        "higher order result: 110",
    ];
    let expected_output = expected_lines.join("\n");
    assert_eq!(
        rust_output, expected_output,
        "Rust VM 输出不符合业务逻辑预期"
    );
}

#[test]
fn test_dsl_cli_invocation_with_aluvm() {
    let alukac_bin = env!("CARGO_BIN_EXE_alukac");
    let aluvm_bin = env!("CARGO_BIN_EXE_aluvm");

    let temp_dir = std::env::temp_dir().join("aluka_dsl_cli_test");
    let _ = fs::create_dir_all(&temp_dir);

    let lisp_file = temp_dir.join("calc.lisp");
    let bc_file = temp_dir.join("calc.bc");

    let lisp_source = r#"
        (def a 100)
        (def b 25)
        (console.log "div:" (/ a b))
    "#;
    fs::write(&lisp_file, lisp_source).expect("写入 lisp 文件失败");

    // 命令行编译
    let comp = Command::new(alukac_bin)
        .arg("compile")
        .arg(&lisp_file)
        .arg("-o")
        .arg(&bc_file)
        .output()
        .expect("alukac 执行失败");
    assert!(comp.status.success());

    // 命令行 aluvm 执行
    let run = Command::new(aluvm_bin)
        .arg("run")
        .arg(&bc_file)
        .output()
        .expect("aluvm 执行失败");
    assert!(run.status.success());

    let stdout_str = String::from_utf8_lossy(&run.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout_str, "div: 4");
}
