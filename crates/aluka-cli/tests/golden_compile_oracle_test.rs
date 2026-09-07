//! 32 个全量黄金语料端到端编译、Verifier 静态校验与 Node.js 22 LTS 运行对拍测试。

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

#[test]
fn test_compile_all_32_golden_corpus_and_verify() {
    let bin_path = env!("CARGO_BIN_EXE_alukac");
    let sources_dir = get_sources_dir();
    assert!(sources_dir.exists(), "黄金语料 sources 目录必须存在");

    let temp_out_dir = std::env::temp_dir().join("alukac_golden_verify");
    let _ = fs::create_dir_all(&temp_out_dir);

    let mut success_count = 0;
    let mut total_count = 0;

    let mut entries: Vec<_> = fs::read_dir(&sources_dir)
        .expect("读取 sources 目录失败")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
        .collect();

    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        total_count += 1;
        let file_path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        let target_bc = temp_out_dir.join(format!("{file_name}.bc"));

        // 1. alukac 编译源文件
        let output = Command::new(bin_path)
            .arg("compile")
            .arg(&file_path)
            .arg("-o")
            .arg(&target_bc)
            .output()
            .expect("执行 alukac 失败");

        assert!(
            output.status.success(),
            "alukac 编译 {file_name} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // 2. 验证生成的 .bc 是否能成功反序列化并 100% 通过 Verifier
        let bc_bytes = fs::read(&target_bc).expect("读取生成的 .bc 失败");
        let module = BytecodeModule::deserialize(&bc_bytes).expect("反序列化模块失败");
        module
            .verify()
            .unwrap_or_else(|e| panic!("{file_name} Verifier 静态校验失败: {e}"));

        success_count += 1;
    }

    println!("32 个黄金语料编译与 Verifier 校验全部通过: {success_count} / {total_count}");
    assert_eq!(success_count, 32, "全部 32 个黄金语料必须全绿通过 Verifier");

    let _ = fs::remove_dir_all(&temp_out_dir);
}

#[test]
fn test_compile_and_execute_node_diff() {
    let bin_path = env!("CARGO_BIN_EXE_alukac");
    let node_bin = match get_node_bin() {
        Some(b) => b,
        None => {
            println!("未检测到 Node.js，跳过对拍比对");
            return;
        }
    };
    let sources_dir = get_sources_dir();

    let temp_out_dir = std::env::temp_dir().join("alukac_golden_node");
    let _ = fs::create_dir_all(&temp_out_dir);

    // 重点挑选覆盖核心语法的关键语料进行端到端双向对拍
    let oracle_cases = [
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

    let mut matched_count = 0;

    for case_name in oracle_cases {
        let src_path = sources_dir.join(format!("{case_name}.js"));
        let target_bc = temp_out_dir.join(format!("{case_name}.bc"));

        // 1. 获取 Node.js 22 的标准输出
        let node_out = Command::new(&node_bin)
            .arg(&src_path)
            .output()
            .unwrap_or_else(|e| panic!("运行 Node.js 失败: {e}"));

        assert!(node_out.status.success(), "Node.js 运行 {case_name} 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .trim()
            .replace("\r\n", "\n");

        // 2. 使用 alukac 编译源文件
        let compile_out = Command::new(bin_path)
            .arg("compile")
            .arg(&src_path)
            .arg("-o")
            .arg(&target_bc)
            .output()
            .expect("执行 alukac compile 失败");

        assert!(compile_out.status.success());

        // 3. 反序列化生成的字节码并送入 VM 执行
        let bc_bytes = fs::read(&target_bc).expect("读取生成的 .bc 失败");
        let module = BytecodeModule::deserialize(&bc_bytes).expect("反序列化模块失败");
        module.verify().expect("Verifier 校验失败");

        let mut vm = Vm::new(0);
        let run_res = vm.run_module(&module);
        if let Err(e) = &run_res {
            println!("  VM 执行出错: {:?}", e);
        }
        let rust_stdout = vm.stdout_records.join("\n").trim().replace("\r\n", "\n");

        // 对比输出是否一致
        assert_eq!(
            rust_stdout, node_stdout,
            "用例 {case_name} 的 Rust 编译产物输出与 Node.js 22 不一致"
        );

        matched_count += 1;
    }

    println!("\nNode.js 22 端到端对拍完成，共成功对拍 {matched_count} 个黄金用例！");
    let _ = fs::remove_dir_all(&temp_out_dir);
}
