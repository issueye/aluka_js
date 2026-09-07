//! alukac 命令行编译器的集成测试。

use std::fs;
use std::process::Command;

#[test]
fn test_alukac_cli_version_and_help() {
    let bin_path = env!("CARGO_BIN_EXE_alukac");

    // 1. --version
    let output = Command::new(bin_path)
        .arg("--version")
        .output()
        .expect("运行 alukac --version 失败");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("alukac"));
    assert!(stdout.contains("Aluka 前端字节码编译器"));

    // 2. --help
    let output = Command::new(bin_path)
        .arg("--help")
        .output()
        .expect("运行 alukac --help 失败");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("用法:"));
    assert!(stdout.contains("alukac compile"));
    assert!(stdout.contains("alukac disasm"));
}

#[test]
fn test_alukac_compile_and_disasm_roundtrip() {
    let bin_path = env!("CARGO_BIN_EXE_alukac");

    // 创建临时测试目录
    let temp_dir = std::env::temp_dir().join("alukac_test_roundtrip");
    let _ = fs::create_dir_all(&temp_dir);

    let js_path = temp_dir.join("sample.js");
    let bc_path = temp_dir.join("sample.bc");

    let source = r#"
        let base = 100;
        function calculate(factor: number): number {
            let temp = base + factor * 2;
            return temp;
        }

        class Engine {
            start() {
                return calculate(5);
            }
        }

        let e = new Engine();
        let result = e.start();
    "#;

    fs::write(&js_path, source).expect("写入临时测试源码失败");

    // 1. 运行编译命令：alukac compile sample.js -o sample.bc
    let compile_output = Command::new(bin_path)
        .arg("compile")
        .arg(&js_path)
        .arg("-o")
        .arg(&bc_path)
        .output()
        .expect("执行 alukac compile 失败");

    assert!(
        compile_output.status.success(),
        "编译应该成功: stderr = {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let compile_stdout = String::from_utf8_lossy(&compile_output.stdout);
    assert!(compile_stdout.contains("编译成功"));

    assert!(bc_path.exists(), "生成的 .bc 字节码文件必须存在");
    let bc_bytes = fs::read(&bc_path).expect("读取生成的 .bc 文件失败");
    assert!(bc_bytes.len() > 20, "字节码文件长度必须大于头部长度");

    // 2. 静态验证生成的字节码
    let module = aluka_bytecode::BytecodeModule::deserialize(&bc_bytes)
        .expect("生成的字节码必须能成功反序列化");
    module
        .verify()
        .expect("生成的字节码必须 100% 通过 Verifier 校验");

    // 3. 运行反汇编命令：alukac disasm sample.bc
    let disasm_output = Command::new(bin_path)
        .arg("disasm")
        .arg(&bc_path)
        .output()
        .expect("执行 alukac disasm 失败");

    assert!(disasm_output.status.success());
    let disasm_stdout = String::from_utf8_lossy(&disasm_output.stdout);
    assert!(disasm_stdout.contains("Aluka 字节码反汇编报告"));
    assert!(disasm_stdout.contains("calculate"));
    assert!(disasm_stdout.contains("Engine"));

    // 4. 测试直接传参默认编译并自动生成 .bc（不带 -o）
    let auto_bc = temp_dir.join("sample_auto.bc");
    let auto_js = temp_dir.join("sample_auto.js");
    fs::write(&auto_js, "let x = 42;").expect("写入 auto_js 失败");

    let direct_output = Command::new(bin_path)
        .arg(&auto_js)
        .output()
        .expect("执行 alukac direct 编译失败");
    assert!(direct_output.status.success());
    assert!(auto_bc.exists(), "必须自动生成同名 .bc 文件");

    // 5. 测试编译错误时返回非零退出码
    let bad_output = Command::new(bin_path)
        .arg("compile")
        .arg("non_existent_file.js")
        .output()
        .expect("执行不存在文件编译失败");
    assert!(!bad_output.status.success());
    let bad_stderr = String::from_utf8_lossy(&bad_output.stderr);
    assert!(bad_stderr.contains("错误: 无法读取源文件"));

    // 清理临时文件
    let _ = fs::remove_file(&js_path);
    let _ = fs::remove_file(&bc_path);
    let _ = fs::remove_file(&auto_js);
    let _ = fs::remove_file(&auto_bc);
    let _ = fs::remove_dir(&temp_dir);
}
