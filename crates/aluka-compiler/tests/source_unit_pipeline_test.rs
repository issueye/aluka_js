//! 源码单元全链路编译与阶段位控制集成测试。

use aluka_compiler::{compile_source_unit, parse_json_to_expr};
use aluka_parser::ast::Expr;
use aluka_parser::source_unit::{
    LanguageRegistry, ModuleKind, STAGE_BYTECODE_COMPILED, STAGE_PARSED, STAGE_TYPE_STRIPPED,
    SourceKind, SourceUnitError,
};

#[test]
fn test_javascript_source_unit_pipeline_and_stage_marking() {
    let src = "function add(a, b) { return a + b; } add(10, 20);";
    let mut unit = LanguageRegistry::global()
        .parse_source(src, "math.js", ModuleKind::Script)
        .expect("解析 JS 单元成功");

    assert_eq!(unit.source_kind, SourceKind::JavaScript);
    unit.require_stages(STAGE_PARSED).expect("应具备解析阶段");
    assert_eq!(unit.stage.0 & STAGE_BYTECODE_COMPILED.0, 0);

    let module = compile_source_unit(&mut unit).expect("编译 JS 单元成功");
    module.verify().expect("生成的字节码应完全符合规范");

    // 验证阶段只增不减推进
    unit.require_stages(STAGE_PARSED | STAGE_BYTECODE_COMPILED)
        .expect("应包含解析与编译完成阶段");

    // 验证防重入：重复标记 STAGE_BYTECODE_COMPILED 应报诊断
    let err = unit
        .mark_stage(STAGE_BYTECODE_COMPILED)
        .expect_err("重复标记阶段应被拦截");
    assert!(matches!(err, SourceUnitError::StageAlreadyApplied { .. }));
}

#[test]
fn test_typescript_source_unit_pipeline() {
    let src = "const num: number = 42; function calc(val: number): number { return val * 2; }";
    let mut unit = LanguageRegistry::global()
        .parse_source(src, "calc.ts", ModuleKind::Script)
        .expect("解析 TS 单元成功");

    assert_eq!(unit.source_kind, SourceKind::TypeScript);
    unit.require_stages(STAGE_PARSED | STAGE_TYPE_STRIPPED)
        .expect("TS 应标记 strip-only 阶段");

    let module = compile_source_unit(&mut unit).expect("编译 TS 单元成功");
    module.verify().expect("TS 编译产物校验通过");
    unit.require_stages(STAGE_BYTECODE_COMPILED)
        .expect("应标记已编译");
}

#[test]
fn test_json_source_unit_pipeline_compiles_data_module() {
    let json_src = r#"{
        "name": "aluka",
        "version": 1.5,
        "features": ["compiler", "vm", "repl"],
        "enabled": true,
        "extra": null
    }"#;

    let mut unit = LanguageRegistry::global()
        .parse_source(json_src, "config.json", ModuleKind::Script)
        .expect("解析 JSON 单元成功");

    assert_eq!(unit.source_kind, SourceKind::Json);
    assert!(unit.program.is_none(), "JSON 单元无常规 JS Program");

    let module = compile_source_unit(&mut unit).expect("JSON 编译为数据模块成功");
    module.verify().expect("JSON 模块字节码校验通过");

    unit.require_stages(STAGE_BYTECODE_COMPILED)
        .expect("JSON 模块编译后应标记已编译阶段");

    assert_eq!(module.functions.len(), 1);
    let main_fn = &module.functions[0];
    assert_eq!(main_fn.name, "main");
    // 验证生成了 NewObject 与 NewArray
    let has_new_obj = main_fn
        .code
        .iter()
        .any(|i| i.op == aluka_bytecode::Op::NewObject);
    let has_new_arr = main_fn
        .code
        .iter()
        .any(|i| i.op == aluka_bytecode::Op::NewArray);
    let has_return = main_fn
        .code
        .iter()
        .any(|i| i.op == aluka_bytecode::Op::Return);
    assert!(has_new_obj, "JSON 对象应生成 NewObject 指令");
    assert!(has_new_arr, "JSON 数组应生成 NewArray 指令");
    assert!(has_return, "JSON 模块应发射 Return 导出数据");
}

#[test]
fn test_parse_json_to_expr_helper() {
    let expr = parse_json_to_expr(r#"[1, "hello", false, null]"#).expect("解析 json 数组");
    match expr {
        Expr::Array(elements) => {
            assert_eq!(elements.len(), 4);
            assert_eq!(elements[0], Expr::Number(1.0));
            assert_eq!(elements[1], Expr::String("hello".to_owned()));
            assert_eq!(elements[2], Expr::Boolean(false));
            assert_eq!(elements[3], Expr::Null);
        }
        _ => panic!("期望 Array 表达式"),
    }
}

#[test]
fn test_custom_language_registry_dispatch() {
    let mut reg = LanguageRegistry::with_defaults();
    reg.register("schema", SourceKind::Json);

    let mut unit = reg
        .parse_source(r#"{"type": "string"}"#, "model.schema", ModuleKind::Script)
        .expect("自定义后缀识别解析");

    assert_eq!(unit.source_kind, SourceKind::Json);
    let module = compile_source_unit(&mut unit).expect("编译自定义 JSON");
    module.verify().expect("校验通过");
}

#[test]
fn test_dsl_source_unit_pipeline() {
    let dsl_src = r#"
        ;; 定义基础变量
        (def x 10)
        (def y 20)
        ;; 高阶函数应用
        (fn add (a b) (+ a b))
        (def res (add x y))
        (console.log "result:" res)
    "#;
    let mut unit = LanguageRegistry::global()
        .parse_source(dsl_src, "test.adsl", ModuleKind::Script)
        .expect("DSL 源码单元解析成功");

    assert_eq!(unit.source_kind, SourceKind::Dsl);
    unit.require_stages(STAGE_PARSED).expect("应标记已解析");

    let module = compile_source_unit(&mut unit).expect("DSL 单元编译成功");
    module.verify().expect("生成的字节码必须通过 ISA 静态校验");
    unit.require_stages(STAGE_BYTECODE_COMPILED)
        .expect("应推进到字节码编译完成阶段");
}
