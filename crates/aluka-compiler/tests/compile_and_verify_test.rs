//! 前端编译器与字节码验证器/VM 端到端联合测试。
//!
//! 验证 aluka-compiler 发射的代码能够通过 Verifier 的 V1..V16 全部严格静态校验，
//! 并且在 VM 中能够被正确求值。

use aluka_bytecode::{BytecodeModule, Op};
use aluka_compiler::{compile, compile_module};
use aluka_parser::{
    ClassMethodDef, Expr, FunctionDef, ObjectProp, Program, PropKey, PropValue, Stmt,
    ast::{SpannedStmt, VarKind},
    parse,
};

#[test]
fn test_compile_arithmetic_and_verify_and_execute() {
    // 构造表达式: (10 + 20) * 3 - 40 / 2
    // 预期值: 30 * 3 - 20 = 90 - 20 = 70
    let expr = Expr::Binary {
        op: "-".to_owned(),
        left: Box::new(Expr::Binary {
            op: "*".to_owned(),
            left: Box::new(Expr::Binary {
                op: "+".to_owned(),
                left: Box::new(Expr::Number(10.0)),
                right: Box::new(Expr::Number(20.0)),
            }),
            right: Box::new(Expr::Number(3.0)),
        }),
        right: Box::new(Expr::Binary {
            op: "/".to_owned(),
            left: Box::new(Expr::Number(40.0)),
            right: Box::new(Expr::Number(2.0)),
        }),
    };

    let program = Program {
        body: vec![SpannedStmt::new(Stmt::Expr(expr), 0)],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_arithmetic");

    // 1. 静态验证：发射的代码必须 100% 通过 Verifier 校验
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module
        .verify()
        .expect("编译器发射的代码必须通过 Verifier 静态校验");

    // 2. 序列化回读验证
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_bitwise_and_unary_and_execute() {
    // 构造表达式: ~( (15 & 7) ^ 2 ) + 10
    // 15 & 7 = 7; 7 ^ 2 = 5; ~5 = -6; -6 + 10 = 4
    let expr = Expr::Binary {
        op: "+".to_owned(),
        left: Box::new(Expr::Unary {
            op: "~".to_owned(),
            expr: Box::new(Expr::Binary {
                op: "^".to_owned(),
                left: Box::new(Expr::Binary {
                    op: "&".to_owned(),
                    left: Box::new(Expr::Number(15.0)),
                    right: Box::new(Expr::Number(7.0)),
                }),
                right: Box::new(Expr::Number(2.0)),
            }),
        }),
        right: Box::new(Expr::Number(10.0)),
    };

    let program = Program {
        body: vec![SpannedStmt::new(Stmt::Expr(expr), 0)],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_bitwise");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module.verify().expect("字节码校验失败");
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_comparison_and_execute() {
    // 构造表达式: (10 < 20) === true
    let expr = Expr::Binary {
        op: "===".to_owned(),
        left: Box::new(Expr::Binary {
            op: "<".to_owned(),
            left: Box::new(Expr::Number(10.0)),
            right: Box::new(Expr::Number(20.0)),
        }),
        right: Box::new(Expr::Boolean(true)),
    };

    let program = Program {
        body: vec![SpannedStmt::new(Stmt::Expr(expr), 0)],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_compare");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module.verify().expect("字节码校验失败");
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_variable_declaration_and_scoping() {
    // 构造程序:
    // let a = 10;
    // let b = 20;
    // a = a + b;
    // a * 2;
    // 预期求值: (10 + 20) * 2 = 60
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "a".to_owned(),
                    init: Some(Expr::Number(10.0)),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "b".to_owned(),
                    init: Some(Expr::Number(20.0)),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Expr(Expr::Assign {
                    name: "a".to_owned(),
                    value: Box::new(Expr::Binary {
                        op: "+".to_owned(),
                        left: Box::new(Expr::Ident("a".to_owned())),
                        right: Box::new(Expr::Ident("b".to_owned())),
                    }),
                }),
                0,
            ),
            SpannedStmt::new(
                Stmt::Expr(Expr::Binary {
                    op: "*".to_owned(),
                    left: Box::new(Expr::Ident("a".to_owned())),
                    right: Box::new(Expr::Number(2.0)),
                }),
                0,
            ),
        ],
    };

    let unit = compile(&program);
    assert_eq!(unit.locals, 2, "应该分配两个局部槽位 a 和 b");
    let func = unit.to_func_template("test_vars");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module.verify().expect("字节码校验失败");
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_control_flow_if_while_and_execute() {
    // 构造程序:
    // let sum = 0;
    // let i = 1;
    // while (i <= 10) {
    //     sum = sum + i;
    //     i = i + 1;
    // }
    // if (sum > 50) {
    //     sum = sum * 2;
    // } else {
    //     sum = 0;
    // }
    // sum;
    // 预期求值: (1+..+10 = 55) > 50 => 55 * 2 = 110
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "sum".to_owned(),
                    init: Some(Expr::Number(0.0)),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "i".to_owned(),
                    init: Some(Expr::Number(1.0)),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::While {
                    cond: Expr::Binary {
                        op: "<=".to_owned(),
                        left: Box::new(Expr::Ident("i".to_owned())),
                        right: Box::new(Expr::Number(10.0)),
                    },
                    body: Box::new(SpannedStmt::new(
                        Stmt::Block(vec![
                            SpannedStmt::new(
                                Stmt::Expr(Expr::Assign {
                                    name: "sum".to_owned(),
                                    value: Box::new(Expr::Binary {
                                        op: "+".to_owned(),
                                        left: Box::new(Expr::Ident("sum".to_owned())),
                                        right: Box::new(Expr::Ident("i".to_owned())),
                                    }),
                                }),
                                0,
                            ),
                            SpannedStmt::new(
                                Stmt::Expr(Expr::Assign {
                                    name: "i".to_owned(),
                                    value: Box::new(Expr::Binary {
                                        op: "+".to_owned(),
                                        left: Box::new(Expr::Ident("i".to_owned())),
                                        right: Box::new(Expr::Number(1.0)),
                                    }),
                                }),
                                0,
                            ),
                        ]),
                        0,
                    )),
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::If {
                    cond: Expr::Binary {
                        op: ">".to_owned(),
                        left: Box::new(Expr::Ident("sum".to_owned())),
                        right: Box::new(Expr::Number(50.0)),
                    },
                    then_branch: Box::new(SpannedStmt::new(
                        Stmt::Expr(Expr::Assign {
                            name: "sum".to_owned(),
                            value: Box::new(Expr::Binary {
                                op: "*".to_owned(),
                                left: Box::new(Expr::Ident("sum".to_owned())),
                                right: Box::new(Expr::Number(2.0)),
                            }),
                        }),
                        0,
                    )),
                    else_branch: Some(Box::new(SpannedStmt::new(
                        Stmt::Expr(Expr::Assign {
                            name: "sum".to_owned(),
                            value: Box::new(Expr::Number(0.0)),
                        }),
                        0,
                    ))),
                },
                0,
            ),
            SpannedStmt::new(Stmt::Expr(Expr::Ident("sum".to_owned())), 0),
        ],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_flow");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module.verify().expect("控制流字节码校验失败");
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_object_array_and_member_access() {
    // 构造测试:
    // let obj = { x: 10, y: 20 };
    // let arr = [obj.x, obj.y, 30];
    // arr[0] + arr[1] + arr[2]; // 10 + 20 + 30 = 60
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "obj".to_owned(),
                    init: Some(Expr::Object(vec![
                        ObjectProp {
                            key: PropKey::Literal("x".to_owned()),
                            value: PropValue::Expr(Expr::Number(10.0)),
                        },
                        ObjectProp {
                            key: PropKey::Literal("y".to_owned()),
                            value: PropValue::Expr(Expr::Number(20.0)),
                        },
                    ])),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "arr".to_owned(),
                    init: Some(Expr::Array(vec![
                        Expr::Member {
                            obj: Box::new(Expr::Ident("obj".to_owned())),
                            prop: "x".to_owned(),
                        },
                        Expr::Member {
                            obj: Box::new(Expr::Ident("obj".to_owned())),
                            prop: "y".to_owned(),
                        },
                        Expr::Number(30.0),
                    ])),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Expr(Expr::Binary {
                    op: "+".to_owned(),
                    left: Box::new(Expr::Binary {
                        op: "+".to_owned(),
                        left: Box::new(Expr::Index {
                            obj: Box::new(Expr::Ident("arr".to_owned())),
                            index: Box::new(Expr::Number(0.0)),
                        }),
                        right: Box::new(Expr::Index {
                            obj: Box::new(Expr::Ident("arr".to_owned())),
                            index: Box::new(Expr::Number(1.0)),
                        }),
                    }),
                    right: Box::new(Expr::Index {
                        obj: Box::new(Expr::Ident("arr".to_owned())),
                        index: Box::new(Expr::Number(2.0)),
                    }),
                }),
                0,
            ),
        ],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_obj_arr");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    module.verify().expect("对象数组字节码校验失败");
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("序列化回读必须成功");
    assert_eq!(module, roundtrip);
}

#[test]
fn test_compile_call_and_method_call_and_new_and_verify() {
    // 构造程序:
    // let fn_val = { ... };
    // let obj = { ... };
    // let ctor = { ... };
    // fn_val(1, 2);
    // obj.compute(3, 4);
    // new ctor(5);
    // return 42;
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "fn_val".to_owned(),
                    init: Some(Expr::Object(vec![])),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "obj".to_owned(),
                    init: Some(Expr::Object(vec![])),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "ctor".to_owned(),
                    init: Some(Expr::Object(vec![])),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::Ident("fn_val".to_owned())),
                    args: vec![Expr::Number(1.0), Expr::Number(2.0)],
                }),
                0,
            ),
            // 方法调用语句：obj.compute(3, 4)
            SpannedStmt::new(
                Stmt::Expr(Expr::MethodCall {
                    receiver: Box::new(Expr::Ident("obj".to_owned())),
                    method: "compute".to_owned(),
                    args: vec![Expr::Number(3.0), Expr::Number(4.0)],
                }),
                0,
            ),
            SpannedStmt::new(
                Stmt::Expr(Expr::New {
                    callee: Box::new(Expr::Ident("ctor".to_owned())),
                    args: vec![Expr::Number(5.0)],
                }),
                0,
            ),
            // 显式返回语句：return 42
            SpannedStmt::new(Stmt::Return(Some(Expr::Number(42.0))), 0),
        ],
    };

    let unit = compile(&program);
    let func = unit.to_func_template("test_calls");

    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };
    // 关键质量门禁：必须 100% 通过 V1..V16 静态校验
    module.verify().expect("函数与方法调用字节码静态校验失败");

    // 验证指令发射
    let ops: Vec<Op> = func.code.iter().map(|i| i.op).collect();
    assert!(ops.contains(&Op::Call), "必须发射 Op::Call");
    assert!(ops.contains(&Op::CallMethod), "必须发射 Op::CallMethod");
    assert!(ops.contains(&Op::New), "必须发射 Op::New");
    assert!(ops.contains(&Op::Return), "必须发射 Op::Return");
}

#[test]
fn test_compile_optional_chaining_and_try_catch_finally_and_verify() {
    // 构造测试程序：
    // let obj = { a: 1 };
    // let val1 = obj?.a;
    // let val2 = obj?.["a"];
    // try {
    //     let x = 10;
    // } catch (e) {
    //     let y = 20;
    // } finally {
    //     let z = 30;
    // }
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "obj".to_owned(),
                    init: Some(Expr::Object(vec![ObjectProp {
                        key: PropKey::Literal("a".to_owned()),
                        value: PropValue::Expr(Expr::Number(1.0)),
                    }])),
                    kind: VarKind::Var,
                },
                0,
            ),
            // 可选链属性访问: obj?.a
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "val1".to_owned(),
                    init: Some(Expr::OptionalMember {
                        obj: Box::new(Expr::Ident("obj".to_owned())),
                        prop: "a".to_owned(),
                    }),
                    kind: VarKind::Var,
                },
                0,
            ),
            // 可选链下标访问: obj?.["a"]
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "val2".to_owned(),
                    init: Some(Expr::OptionalIndex {
                        obj: Box::new(Expr::Ident("obj".to_owned())),
                        index: Box::new(Expr::String("a".to_owned())),
                    }),
                    kind: VarKind::Var,
                },
                0,
            ),
            // Try-Catch-Finally 结构
            SpannedStmt::new(
                Stmt::Try {
                    body: Box::new(SpannedStmt::new(
                        Stmt::VarDecl {
                            name: "x".to_owned(),
                            init: Some(Expr::Number(10.0)),
                            kind: VarKind::Var,
                        },
                        0,
                    )),
                    catch_param: Some("e".to_owned()),
                    catch_body: Some(Box::new(SpannedStmt::new(
                        Stmt::VarDecl {
                            name: "y".to_owned(),
                            init: Some(Expr::Number(20.0)),
                            kind: VarKind::Var,
                        },
                        0,
                    ))),
                    finally_body: Some(Box::new(SpannedStmt::new(
                        Stmt::VarDecl {
                            name: "z".to_owned(),
                            init: Some(Expr::Number(30.0)),
                            kind: VarKind::Var,
                        },
                        0,
                    ))),
                },
                0,
            ),
            SpannedStmt::new(Stmt::Return(Some(Expr::Number(100.0))), 0),
        ],
    };

    let unit = compile(&program);
    assert!(!unit.try_table.is_empty(), "必须生成至少一条 TryEntry");
    let entry = &unit.try_table[0];
    assert!(entry.has_catch, "必须标记 has_catch");
    assert!(entry.has_finally, "必须标记 has_finally");
    assert!(
        entry.start_pc < entry.end_pc,
        "V11: start_pc 必须小于 end_pc"
    );
    assert_eq!(entry.start_pc % 4, 0, "V14: start_pc 必须 4 字节对齐");
    assert_eq!(entry.end_pc % 4, 0, "V14: end_pc 必须 4 字节对齐");

    let func = unit.to_func_template("test_opt_try");
    let module = BytecodeModule {
        header_extras: Vec::new(),
        version: 30,
        functions: vec![func.clone()],
        classes: Vec::new(),
    };

    // 关键质量门禁：数据流与异常表必须 100% 通过 Verifier 的 V1..V16 严格检查
    module
        .verify()
        .expect("可选链与 Try-Catch-Finally 字节码静态校验失败");

    // 验证指令发射
    let ops: Vec<Op> = func.code.iter().map(|i| i.op).collect();
    assert!(ops.contains(&Op::OptionalJump), "必须发射 Op::OptionalJump");
}

#[test]
fn test_compile_module_with_classes_and_functions_and_verify() {
    // 构造模块:
    // class Base {
    //     constructor(name) { ... }
    //     greet() { return 1; }
    // }
    // class Derived extends Base {
    //     constructor(name) { ... }
    //     calc() { return 2; }
    // }
    // function helper(x) { return x; }
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::Class {
                    name: "Base".to_owned(),
                    super_class: None,
                    constructor: Some(FunctionDef::new(
                        "Base_constructor".to_owned(),
                        vec!["name".to_owned()],
                        false,
                        vec![SpannedStmt::new(Stmt::Return(Some(Expr::Number(0.0))), 0)],
                    )),
                    methods: vec![ClassMethodDef {
                        name: "greet".to_owned(),
                        params: Vec::new(),
                        body: vec![SpannedStmt::new(Stmt::Return(Some(Expr::Number(1.0))), 0)],
                        is_static: false,
                        kind: 0,
                    }],
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Class {
                    name: "Derived".to_owned(),
                    super_class: Some(Expr::Ident("Base".to_owned())),
                    constructor: Some(FunctionDef::new(
                        "Derived_constructor".to_owned(),
                        vec!["name".to_owned()],
                        false,
                        vec![SpannedStmt::new(Stmt::Return(Some(Expr::Number(0.0))), 0)],
                    )),
                    methods: vec![ClassMethodDef {
                        name: "calc".to_owned(),
                        params: Vec::new(),
                        body: vec![SpannedStmt::new(Stmt::Return(Some(Expr::Number(2.0))), 0)],
                        is_static: false,
                        kind: 0,
                    }],
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Function(FunctionDef::new(
                    "helper".to_owned(),
                    vec!["x".to_owned()],
                    false,
                    vec![SpannedStmt::new(
                        Stmt::Return(Some(Expr::Ident("x".to_owned()))),
                        0,
                    )],
                )),
                0,
            ),
        ],
    };

    let module = compile_module(&program);

    assert_eq!(module.version, 30, "模块版本必须为 30");
    assert_eq!(module.classes.len(), 2, "必须成功生成 2 个类模板");
    assert!(!module.classes[0].has_super, "Base 类无父类");
    assert!(module.classes[1].has_super, "Derived 类继承 Base");
    assert!(
        module.functions.len() >= 5,
        "必须包含顶层函数、2 个构造函数、2 个方法模板"
    );

    // 关键质量门禁：验证多函数、类模板及其交叉索引完全通过 Verifier 静态校验（V15 模板越界等规则）
    module
        .verify()
        .expect("编译出的模块化类与函数字节码必须完全通过 Verifier 校验");

    // 关键质量门禁：验证编译生成的 BytecodeModule 能够被二进制序列化器无损序列化并再次回读
    let serialized = module.serialize();
    assert!(!serialized.is_empty(), "序列化二进制数据不得为空");
    let roundtrip = BytecodeModule::deserialize(&serialized)
        .expect("序列化后的字节流必须能被 deserialize 正确反序列化");
    assert_eq!(module, roundtrip, "编译产物序列化再反序列化必须严格等价");
}

#[test]
fn test_compile_nested_closure_upvalue_capture_and_verify() {
    // 构造测试程序：
    // let factor = 10;
    // function makeMultiplier(base) {
    //     function multiplier(val) {
    //         return val * base;
    //     }
    //     return multiplier;
    // }
    let program = Program {
        body: vec![
            SpannedStmt::new(
                Stmt::VarDecl {
                    name: "factor".to_owned(),
                    init: Some(Expr::Number(10.0)),
                    kind: VarKind::Var,
                },
                0,
            ),
            SpannedStmt::new(
                Stmt::Function(FunctionDef::new(
                    "makeMultiplier".to_owned(),
                    vec!["base".to_owned()],
                    false,
                    vec![
                        SpannedStmt::new(
                            Stmt::Function(FunctionDef::new(
                                "multiplier".to_owned(),
                                vec!["val".to_owned()],
                                false,
                                vec![SpannedStmt::new(
                                    Stmt::Return(Some(Expr::Binary {
                                        op: "*".to_owned(),
                                        left: Box::new(Expr::Ident("val".to_owned())),
                                        right: Box::new(Expr::Ident("base".to_owned())),
                                    })),
                                    0,
                                )],
                            )),
                            0,
                        ),
                        SpannedStmt::new(
                            Stmt::Return(Some(Expr::Ident("multiplier".to_owned()))),
                            0,
                        ),
                    ],
                )),
                0,
            ),
        ],
    };

    let module = compile_module(&program);

    // 验证包含顶层脚本函数、makeMultiplier 与 inner multiplier
    assert_eq!(module.functions.len(), 3, "必须编译出 3 个函数模板");

    // multiplier 必须有 1 个 upvalue 捕获项（base）
    let multiplier_func = module
        .functions
        .iter()
        .find(|f| f.name == "multiplier")
        .expect("必须包含 multiplier 函数模板");
    assert_eq!(
        multiplier_func.upvalues.len(),
        1,
        "multiplier 必须捕获 1 个 Upvalue"
    );
    assert!(
        multiplier_func.upvalues[0].is_local,
        "捕获直接父级的局部变量"
    );
    // 验证 multiplier 指令序列中包含 Op::LoadUpvalue
    let ops: Vec<Op> = multiplier_func.code.iter().map(|i| i.op).collect();
    assert!(
        ops.contains(&Op::LoadUpvalue),
        "必须发射 Op::LoadUpvalue 读取闭包变量"
    );

    // 关键质量门禁：必须 100% 通过 Verifier V16 上值边界校验
    module
        .verify()
        .expect("闭包上值捕获必须 100% 通过 Verifier 严格校验");

    // 关键质量门禁：验证闭包模块能无损序列化并回读
    let bytes = module.serialize();
    let roundtrip = BytecodeModule::deserialize(&bytes).expect("闭包模块回读必须成功");
    assert_eq!(module, roundtrip, "闭包模块序列化回读必须完全一致");
}

#[test]
fn test_parse_and_compile_source_text_to_bytecode_and_verify() {
    // 直接输入包含 TypeScript 注解、类继承、可选链短路与 Try-Catch 的真实源代码
    let ts_source = r#"
        // 声明带类型注解的变量与接口
        interface Point { x: number; y: number; }
        type ID = string | number;

        let baseVal: number = 100;
        const multiplier = 2;

        function calc(extra: number): number {
            let total = baseVal + extra;
            return total;
        }

        class Calculator extends Object {
            constructor(name: string) {
                this.name = name;
            }
            compute(val: number) {
                return val * multiplier;
            }
        }

        try {
            let calcObj = new Calculator("my_calc");
            let ans = calcObj?.compute(42);
        } catch (err: any) {
            let fallback = 0;
        } finally {
            let done = 1;
        }
    "#;

    // 1. 词法与语法分析（含 TS 类型注解零成本剥离）
    let program = parse(ts_source);
    println!("Program body: {:#?}", program);
    assert!(!program.body.is_empty(), "解析出的 AST 不得为空");

    // 2. 模块级编译与字节码生成
    let module = compile_module(&program);

    assert_eq!(module.version, 30, "模块版本必须为 30");
    assert_eq!(module.classes.len(), 1, "必须生成 1 个类模板");
    assert!(module.classes[0].has_super, "Calculator 继承 Object");
    assert!(
        module.functions.len() >= 4,
        "至少包含 main, calc, ctor, compute"
    );

    // 3. 严格执行 Verifier V1..V16 静态安全检查
    module
        .verify()
        .expect("从源码文本编译出的字节码模块必须 100% 通过 Verifier 静态校验");

    // 4. 二进制序列化与无损 Round-trip 验证
    let bc_bytes = module.serialize();
    assert!(!bc_bytes.is_empty(), "序列化二进制不得为空");
    let roundtrip = BytecodeModule::deserialize(&bc_bytes).expect("二进制反序列化必须成功");
    assert_eq!(module, roundtrip, "从源码生成的模块必须实现二进制无损回读");
}

#[test]
fn test_compile_arguments_allocates_slot_and_marks_extras() {
    // arguments 引用 → 分配隐藏局部槽并写 header_extras；不引用则不分配
    let with_args =
        parse("function f(a, b) { return arguments.length + a; }\nconsole.log(f(1, 2));");
    let m1 = compile_module(&with_args);
    // main(0) 无 arguments；f(1) 引用 arguments
    let f_idx = m1
        .functions
        .iter()
        .position(|f| f.name == "f")
        .expect("f 函数存在");
    let extras = m1.header_extras.get(f_idx).expect("f 应有 header_extras");
    assert!(
        extras.arguments_slot >= 0,
        "引用 arguments 的函数必须分配槽位（实际 {extras:?}）"
    );
    assert!(
        !extras.no_arguments_object,
        "引用 arguments 不得标记 no_arguments"
    );
    // 槽位必须落在 num_locals 内
    assert!(
        (extras.arguments_slot as usize) < m1.functions[f_idx].num_locals as usize,
        "arguments 槽位必须在 num_locals 内"
    );

    let no_args = parse("function g(x) { return x * 2; }\ng(3);");
    let m2 = compile_module(&no_args);
    let g_idx = m2
        .functions
        .iter()
        .position(|f| f.name == "g")
        .expect("g 函数存在");
    assert!(
        m2.header_extras
            .get(g_idx)
            .is_none_or(|e| e.arguments_slot == -1),
        "不引用 arguments 的函数不分配槽位（或 extras 归空）"
    );
    m2.verify().expect("编译产物必须通过 Verifier");
    m1.verify().expect("编译产物必须通过 Verifier");
}

#[test]
fn test_compile_arguments_shadowed_by_local_and_arrow() {
    // 函数内 let arguments 遮蔽 → 不分配 arguments 槽；箭头函数不绑定 arguments
    let shadowed = parse("function h() { let arguments = 1; return arguments; }\nh();");
    let m1 = compile_module(&shadowed);
    let h_idx = m1
        .functions
        .iter()
        .position(|f| f.name == "h")
        .expect("h 函数存在");
    assert!(
        m1.header_extras
            .get(h_idx)
            .is_none_or(|e| e.arguments_slot == -1),
        "被遮蔽时不得分配 arguments 槽"
    );

    // 箭头函数引用 arguments → 不绑定自己的槽，交由外层函数分配
    let arrow = parse("function outer(a) { return (() => arguments.length)(); }\nouter(1);");
    let m2 = compile_module(&arrow);
    let outer_idx = m2
        .functions
        .iter()
        .position(|f| f.name == "outer")
        .expect("outer 函数存在");
    let arrow_idx = m2
        .functions
        .iter()
        .position(|f| f.name.is_empty())
        .expect("箭头函数模板存在");
    assert!(
        m2.header_extras[outer_idx].arguments_slot >= 0,
        "外层函数应分配 arguments 槽供箭头捕获"
    );
    assert_eq!(
        m2.header_extras[arrow_idx].arguments_slot, -1,
        "箭头函数不得绑定自己的 arguments 槽"
    );
    m2.verify().expect("编译产物必须通过 Verifier");
}

#[test]
fn test_compile_keyword_member_access_is_preserved() {
    // 关键字作属性名（如 `m.default`）不得被成员访问解析吞掉
    let program = parse("function f(m) { return m.default; }\nf({});");
    let module = compile_module(&program);
    module.verify().expect("通过 Verifier");
    let f = module
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("f 函数存在");
    assert!(
        f.constants
            .iter()
            .any(|c| matches!(c, aluka_bytecode::Constant::String(s) if s == "default")),
        "`m.default` 必须编译出 default 属性访问（实际常量池 {f:?}）"
    );
}
