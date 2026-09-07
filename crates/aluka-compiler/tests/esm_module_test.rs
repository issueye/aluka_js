//! ESM 模块编译集成测试。

use aluka_compiler::compile_module;
use aluka_parser::parse;

#[test]
fn test_esm_import_and_export_compilation() {
    let src = r#"
        import { add } from './math';
        import React from 'react';
        import 'reset.css';

        export const base = 10;
        export function double(x) {
            return x * 2;
        }
        export default function run() {
            return double(base);
        }
    "#;

    let program = parse(src);
    let module = compile_module(&program);
    module
        .verify()
        .expect("包含 ESM import 与 export 的模块应顺利通过编译与字节码校验");

    // 验证顶层主函数生成并包含了局部变量与函数
    assert!(module.functions.len() >= 3);
    assert_eq!(module.functions[0].name, "main");
}
