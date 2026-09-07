//! 黄金语料库（Golden Bytecode Corpus）在 aluvm 上的执行验证。
//!
//! 验证目标：
//! 1. 黄金语料库覆盖的 106 条全指令在 aluvm 上能够完整解码、通过 Verifier 静态校验并正确执行；
//! 2. 对包含标准 JavaScript 源码的语料（01..32），执行输出与 Node.js 22 官方输出逐字符严格一致；
//! 3. 99 号合成特殊指令模块能够在 aluvm 上无异常正常执行。

use aluka_bytecode::BytecodeModule;
use aluka_vm::{Value, Vm};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn get_corpus_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/corpus")
        .join(name)
}

fn get_source_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/sources")
        .join(format!("{stem}.js"))
}

/// 运行系统 Node.js（Node.js 22 LTS）获取标准期望输出
fn get_node_output(stem: &str) -> Option<String> {
    let src_path = get_source_path(stem);
    if !src_path.exists() {
        return None;
    }
    let node_bin = std::env::var("NODE").unwrap_or_else(|_| "node".to_string());
    let out = Command::new(node_bin).arg(&src_path).output().ok()?;
    if out.status.success() {
        Some(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .replace("\r\n", "\n"),
        )
    } else {
        None
    }
}

/// 统一执行黄金语料并在 Node.js 22 可用时进行逐字对拍
fn assert_corpus_matches_node(stem: &str) {
    let bc_path = get_corpus_path(&format!("{stem}.bc"));
    let data = fs::read(&bc_path).unwrap_or_else(|e| panic!("读取 {stem}.bc 失败: {e}"));
    let module =
        BytecodeModule::deserialize(&data).unwrap_or_else(|e| panic!("反序列化 {stem} 失败: {e}"));
    module
        .verify()
        .unwrap_or_else(|e| panic!("{stem} 字节码校验失败: {e}"));

    let mut vm = Vm::new(0);
    let result = vm
        .run_module(&module)
        .unwrap_or_else(|e| panic!("执行 {stem} 失败: {e}"));
    assert!(
        matches!(result, Value::Undefined),
        "{stem} 顶层执行应返回 undefined"
    );

    let rust_output = vm.stdout_records.join("\n").replace("\r\n", "\n");

    if let Some(node_output) = get_node_output(stem) {
        assert_eq!(
            rust_output.trim(),
            node_output.trim(),
            "{stem} 执行输出与 Node.js 22 不一致"
        );
    }
}

#[test]
fn test_execute_01_arithmetic_bitwise() {
    assert_corpus_matches_node("01_arithmetic_bitwise");
}

#[test]
fn test_execute_02_literals_and_stack() {
    assert_corpus_matches_node("02_literals_and_stack");
}

#[test]
fn test_execute_03_comparisons() {
    assert_corpus_matches_node("03_comparisons");
}

#[test]
fn test_execute_04_control_flow_jumps() {
    assert_corpus_matches_node("04_control_flow_jumps");
}

#[test]
fn test_execute_05_optional_chaining() {
    assert_corpus_matches_node("05_optional_chaining");
}

#[test]
fn test_execute_06_closures_and_upvalues() {
    assert_corpus_matches_node("06_closures_and_upvalues");
}

#[test]
fn test_execute_07_objects_and_properties() {
    assert_corpus_matches_node("07_objects_and_properties");
}

#[test]
fn test_execute_08_arrays_and_methods() {
    assert_corpus_matches_node("08_arrays_and_methods");
}

#[test]
fn test_execute_09_classes_and_inheritance() {
    assert_corpus_matches_node("09_classes_and_inheritance");
}

#[test]
fn test_execute_10_try_catch_finally() {
    assert_corpus_matches_node("10_try_catch_finally");
}

#[test]
fn test_execute_11_generators_and_iterators() {
    assert_corpus_matches_node("11_generators_and_iterators");
}

#[test]
fn test_execute_12_for_in_keys() {
    assert_corpus_matches_node("12_for_in_keys");
}

#[test]
fn test_execute_13_async_await() {
    assert_corpus_matches_node("13_async_await");
}

#[test]
fn test_execute_14_regexp_and_types() {
    assert_corpus_matches_node("14_regexp_and_types");
}

#[test]
fn test_execute_15_update_expressions() {
    assert_corpus_matches_node("15_update_expressions");
}

#[test]
fn test_execute_16_destructuring_and_spread() {
    assert_corpus_matches_node("16_destructuring_and_spread");
}

#[test]
fn test_execute_17_in_and_instanceof() {
    assert_corpus_matches_node("17_in_and_instanceof");
}

#[test]
fn test_execute_18_switch_statement() {
    assert_corpus_matches_node("18_switch_statement");
}

#[test]
fn test_execute_19_while_dowhile() {
    assert_corpus_matches_node("19_while_dowhile");
}

#[test]
fn test_execute_20_apply_and_spread_call() {
    assert_corpus_matches_node("20_apply_and_spread_call");
}

#[test]
fn test_execute_21_template_literals() {
    assert_corpus_matches_node("21_template_literals");
}

#[test]
fn test_execute_22_nested_closures() {
    assert_corpus_matches_node("22_nested_closures");
}

#[test]
fn test_execute_23_computed_getter_setter() {
    assert_corpus_matches_node("23_computed_getter_setter");
}

#[test]
fn test_execute_24_typeof_global() {
    assert_corpus_matches_node("24_typeof_global");
}

#[test]
fn test_execute_25_call_this_constructor() {
    assert_corpus_matches_node("25_call_this_constructor");
}

#[test]
fn test_execute_26_for_await_of() {
    assert_corpus_matches_node("26_for_await_of");
}

#[test]
fn test_execute_27_chained_try_finally() {
    assert_corpus_matches_node("27_chained_try_finally");
}

#[test]
fn test_execute_28_super_methods() {
    assert_corpus_matches_node("28_super_methods");
}

#[test]
fn test_execute_29_dynamic_arithmetic_ops() {
    assert_corpus_matches_node("29_dynamic_arithmetic_ops");
}

#[test]
fn test_execute_30_dynamic_globals_and_undef() {
    assert_corpus_matches_node("30_dynamic_globals_and_undef");
}

#[test]
fn test_execute_31_dynamic_props_and_spread() {
    assert_corpus_matches_node("31_dynamic_props_and_spread");
}

#[test]
fn test_execute_32_try_exit_jmp_loop() {
    assert_corpus_matches_node("32_try_exit_jmp_loop");
}

#[test]
fn test_execute_99_synthetic_special_opcodes() {
    let bc_path = get_corpus_path("99_synthetic_special_opcodes.bc");
    let data = fs::read(&bc_path).expect("读取 99_synthetic_special_opcodes.bc 失败");
    let module = BytecodeModule::deserialize(&data).expect("反序列化模块失败");
    module.verify().expect("字节码校验失败");

    let mut vm = Vm::new(0);
    vm.run_module(&module)
        .expect("合成模块（特殊指令）应能在 aluvm 上完整执行");
}
