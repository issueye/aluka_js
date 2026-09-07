//! 双向四象限质量对拍矩阵自动化测试套件。
//!
//! 四象限覆盖：
//! - 象限 1 (Q1: Rust 前端编译 ➔ Rust VM 执行)
//! - 象限 2 (Q2: Rust 前端关闭优化编译 ➔ Rust VM 执行)
//! - 象限 3 (Q3: 持久化字节码语料 corpus/*.bc ➔ Rust VM 执行)
//! - 象限 4 (Q4: Node.js 22 LTS 官方引擎标准执行)
//!
//! 断言四象限对拍矩阵在 32 个真实黄金语料上 100% 逐字输出完全一致。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use aluka_bytecode::BytecodeModule;
use aluka_vm::Vm;

fn get_sources_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("golden")
        .join("sources")
}

fn get_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("golden")
        .join("corpus")
}

fn get_node_bin() -> Option<String> {
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_string());
    if Command::new(&node)
        .arg("-v")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        Some(node)
    } else {
        None
    }
}

const GOLDEN_CASES: [&str; 32] = [
    "01_arithmetic_bitwise",
    "02_literals_and_stack",
    "03_comparisons",
    "04_control_flow_jumps",
    "05_optional_chaining",
    "06_closures_and_upvalues",
    "07_objects_and_properties",
    "08_arrays_and_methods",
    "09_classes_and_inheritance",
    "10_try_catch_finally",
    "11_generators_and_iterators",
    "12_for_in_keys",
    "13_async_await",
    "14_regexp_and_types",
    "15_update_expressions",
    "16_destructuring_and_spread",
    "17_in_and_instanceof",
    "18_switch_statement",
    "19_while_dowhile",
    "20_apply_and_spread_call",
    "21_template_literals",
    "22_nested_closures",
    "23_computed_getter_setter",
    "24_typeof_global",
    "25_call_this_constructor",
    "26_for_await_of",
    "27_chained_try_finally",
    "28_super_methods",
    "29_dynamic_arithmetic_ops",
    "30_dynamic_globals_and_undef",
    "31_dynamic_props_and_spread",
    "32_try_exit_jmp_loop",
];

#[test]
fn test_four_quadrants_oracle_matrix() {
    let alukac_bin = env!("CARGO_BIN_EXE_alukac");
    let sources_dir = get_sources_dir();
    let corpus_dir = get_corpus_dir();
    let node_bin = match get_node_bin() {
        Some(n) => n,
        None => {
            println!("未检测到 Node.js，跳过四象限对拍");
            return;
        }
    };

    let temp_dir = std::env::temp_dir().join("aluka_four_quadrants_matrix");
    let _ = fs::create_dir_all(&temp_dir);

    let mut success_cases = 0;

    for case_name in GOLDEN_CASES {
        let js_path = sources_dir.join(format!("{case_name}.js"));
        let corpus_bc_path = corpus_dir.join(format!("{case_name}.bc"));
        let rust_bc_path = temp_dir.join(format!("rust_{case_name}.bc"));
        let rust_no_opt_bc_path = temp_dir.join(format!("rust_no_opt_{case_name}.bc"));

        // ============================================================
        // 象限 1 (Q1: Rust 前端常规编译 ➔ Rust VM)
        // ============================================================
        let compile_output = Command::new(alukac_bin)
            .arg("compile")
            .arg(&js_path)
            .arg("-o")
            .arg(&rust_bc_path)
            .output()
            .expect("alukac 编译失败");
        assert!(
            compile_output.status.success(),
            "alukac 编译 {case_name} 失败"
        );

        let rust_bc_bytes = fs::read(&rust_bc_path).expect("读取 rust.bc 失败");
        let rust_module =
            BytecodeModule::deserialize(&rust_bc_bytes).expect("反序列化 Rust 编译产物失败");
        rust_module.verify().expect("Rust 编译产物静态校验未通过");

        let mut vm_q1 = Vm::new(0);
        let _ = vm_q1.run_module(&rust_module);
        let q1_output = vm_q1.stdout_records.join("\n").trim().replace("\r\n", "\n");

        // ============================================================
        // 象限 2 (Q2: Rust 前端关闭优化 ➔ Rust VM)
        // ============================================================
        let compile_no_opt = Command::new(alukac_bin)
            .arg("compile")
            .arg(&js_path)
            .arg("-o")
            .arg(&rust_no_opt_bc_path)
            .arg("--no-opt")
            .output()
            .expect("alukac --no-opt 编译失败");
        assert!(compile_no_opt.status.success());
        let no_opt_bytes = fs::read(&rust_no_opt_bc_path).expect("读取 no_opt.bc 失败");
        let no_opt_module = BytecodeModule::deserialize(&no_opt_bytes).unwrap();
        no_opt_module.verify().unwrap();
        let mut vm_q2 = Vm::new(0);
        let _ = vm_q2.run_module(&no_opt_module);
        let q2_output = vm_q2.stdout_records.join("\n").trim().replace("\r\n", "\n");

        // ============================================================
        // 象限 3 (Q3: 持久化语料库 ➔ Rust VM)
        // ============================================================
        let corpus_bytes = fs::read(&corpus_bc_path).expect("读取 corpus.bc 失败");
        let corpus_module = BytecodeModule::deserialize(&corpus_bytes).expect("反序列化语料失败");
        corpus_module.verify().expect("语料静态校验未通过");
        let mut vm_q3 = Vm::new(0);
        let _ = vm_q3.run_module(&corpus_module);
        let q3_output = vm_q3.stdout_records.join("\n").trim().replace("\r\n", "\n");

        // ============================================================
        // 象限 4 (Q4: Node.js 22 LTS 官方标准执行)
        // ============================================================
        let node_out = Command::new(&node_bin)
            .arg(&js_path)
            .output()
            .expect("执行 Node.js 失败");
        assert!(node_out.status.success(), "Node.js 执行 {case_name} 失败");
        let q4_output = String::from_utf8_lossy(&node_out.stdout)
            .trim()
            .replace("\r\n", "\n");

        // ============================================================
        // 四象限一致性断言
        // ============================================================
        assert_eq!(
            q1_output, q4_output,
            "[{case_name}] 象限 1 (Rust编译) 与象限 4 (Node.js 22) 不一致!\nQ1: {q1_output}\nQ4: {q4_output}"
        );
        assert_eq!(
            q2_output, q4_output,
            "[{case_name}] 象限 2 (无优化编译) 与象限 4 (Node.js 22) 不一致!\nQ2: {q2_output}\nQ4: {q4_output}"
        );
        assert_eq!(
            q3_output, q4_output,
            "[{case_name}] 象限 3 (语料字节码) 与象限 4 (Node.js 22) 不一致!\nQ3: {q3_output}\nQ4: {q4_output}"
        );

        println!(
            "✓ [四象限通过] {case_name:<30} | Q1={:<4} Q2={:<4} Q3={:<4} Q4={:<4}",
            q1_output.len(),
            q2_output.len(),
            q3_output.len(),
            q4_output.len()
        );
        success_cases += 1;
    }

    println!("\n============================================================");
    println!(
        "四象限交叉矩阵对拍完成: 全部 {} / 32 个真实黄金语料 100% 逐字全绿通过！",
        success_cases
    );
    println!("============================================================");
    assert_eq!(success_cases, 32);

    let _ = fs::remove_dir_all(&temp_dir);
}
