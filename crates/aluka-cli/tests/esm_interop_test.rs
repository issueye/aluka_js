//! `require(esm)` 互操作回归：ESM 模块按 CJS 包装形态编译，`require` 返回
//! 带命名/默认导出与 `__esModule` 标记的 exports 对象（对齐 node22 口径）。

use aluka_bytecode::BytecodeModule;
use aluka_compiler::{compile_esm_module, compile_module};
use aluka_parser::parse;
use aluka_vm::Vm;
use std::path::Path;

#[test]
fn require_esm_returns_namespace_with_named_and_default() {
    let dir = std::env::temp_dir().join(format!("aluka_esm_interop_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录");

    // 1. ESM 依赖模块：命名导出 + 默认导出
    let esm = parse("export const named = 42;\nexport default function greet() { return 'hi'; }");
    let esm_module = compile_esm_module(&esm);
    esm_module.verify().expect("ESM 模块通过 Verifier");
    let esm_bc = dir.join("dep.esm.bc");
    std::fs::write(&esm_bc, esm_module.serialize()).unwrap();

    // 2. CJS 主模块：require(esm) 并输出互操作观察值
    let main_src = parse(
        "const m = require('./dep.esm.bc');\n\
         console.log('result: ' + m.named + '|' + (typeof m.default) + '|' +\n\
           Object.keys(m).sort().join(','));",
    );
    let main_module = compile_module(&main_src);
    main_module.verify().expect("主模块通过 Verifier");
    let main_bc = dir.join("main.bc");
    std::fs::write(&main_bc, main_module.serialize()).unwrap();

    // 3. 执行：setup_cjs 以 main.bc 为入口，require 相对解析同目录依赖
    let mut vm = Vm::new(0);
    vm.setup_cjs(Path::new(&main_bc));
    let data = std::fs::read(&main_bc).unwrap();
    let (loaded, payload) = BytecodeModule::load_any_container(&data).unwrap();
    vm.load_module(&data[payload], &loaded).expect("load");
    vm.run_module(&loaded).expect("run");

    assert_eq!(
        vm.stdout_records,
        vec!["result: 42|function|__esModule,default,named".to_owned()],
        "require(esm) 的命名/默认导出与 __esModule 标记必须与 node22 一致"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
