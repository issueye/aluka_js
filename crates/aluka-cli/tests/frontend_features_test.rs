//! 前端现代语言特性端到端集成测试。
//!
//! 验证此前评审登记的三项核心缺陷修复效果：
//! 1. 模板字符串插值（${} 表达式计算与自动拼接）
//! 2. 派生类默认构造函数自动转发 super(...args)
//! 3. 高级解构模式（对象/数组默认值回退与函数形参解构降级）

use aluka_runtime::Runtime;

#[test]
fn test_template_literal_interpolation() {
    let mut runtime = Runtime::new();
    let src = r#"
        const name = "Aluka";
        const a = 10;
        const b = 20;
        console.log(`Hello, ${name}! Sum: ${a + b}`);

        // 嵌套模板插值与无插值
        const plain = `plain text`;
        const nested = `outer: [${`inner: ${1 + 1}`}]`;
        console.log(plain);
        console.log(nested);
    "#;

    let res = runtime
        .execute_source(src, "template.js", &[], true)
        .expect("模板字符串执行成功");
    assert_eq!(res, aluka_vm::Value::Undefined);

    let stdout = runtime.stdout_records();
    assert_eq!(stdout.len(), 3);
    assert_eq!(stdout[0], "Hello, Aluka! Sum: 30");
    assert_eq!(stdout[1], "plain text");
    assert_eq!(stdout[2], "outer: [inner: 2]");
}

#[test]
fn test_class_derived_default_constructor_super_forwarding() {
    let mut runtime = Runtime::new();
    let src = r#"
        class Animal {
            constructor(name, age) {
                this.name = name;
                this.age = age;
            }
            getInfo() {
                return `${this.name} is ${this.age} years old`;
            }
        }

        // 子类省略 constructor，应自动合成 constructor(...args) { super(...args); }
        class Dog extends Animal {}

        const dog = new Dog("Buddy", 3);
        console.log(dog.name);
        console.log(dog.age);
        console.log(dog.getInfo());
    "#;

    let res = runtime
        .execute_source(src, "class_inherit.js", &[], true)
        .expect("类继承执行成功");
    assert_eq!(res, aluka_vm::Value::Undefined);

    let stdout = runtime.stdout_records();
    assert_eq!(stdout.len(), 3);
    assert_eq!(stdout[0], "Buddy");
    assert_eq!(stdout[1], "3");
    assert_eq!(stdout[2], "Buddy is 3 years old");
}

#[test]
fn test_advanced_destructuring_and_defaults() {
    let mut runtime = Runtime::new();
    let src = r#"
        // 1. 对象解构默认值与别名
        const { a = 1, b: renamed = 2, c = 3 } = { c: 30 };
        console.log(a, renamed, c);

        // 2. 数组解构默认值与 rest
        const [x = 10, y = 20, ...rest] = [100];
        console.log(x, y, rest.length);

        // 3. 嵌套解构
        const { meta: { title = "Untitled" } } = { meta: {} };
        console.log(title);
    "#;

    let res = runtime
        .execute_source(src, "destruct.js", &[], true)
        .expect("高级解构执行成功");
    assert_eq!(res, aluka_vm::Value::Undefined);

    let stdout = runtime.stdout_records();
    assert_eq!(stdout.len(), 3);
    assert_eq!(stdout[0], "1 2 30");
    assert_eq!(stdout[1], "100 20 0");
    assert_eq!(stdout[2], "Untitled");
}

#[test]
fn test_function_parameter_destructuring() {
    let mut runtime = Runtime::new();
    let src = r#"
        function printUser({ name = "anonymous", age = 18 }) {
            console.log(`${name}: ${age}`);
        }

        function printCoords([x = 0, y = 0]) {
            console.log(`x=${x}, y=${y}`);
        }

        printUser({ name: "Alice", age: 25 });
        printUser({});
        printCoords([10, 20]);
        printCoords([]);
    "#;

    let res = runtime
        .execute_source(src, "params.js", &[], true)
        .expect("形参解构执行成功");
    assert_eq!(res, aluka_vm::Value::Undefined);

    let stdout = runtime.stdout_records();
    assert_eq!(stdout.len(), 4);
    assert_eq!(stdout[0], "Alice: 25");
    assert_eq!(stdout[1], "anonymous: 18");
    assert_eq!(stdout[2], "x=10, y=20");
    assert_eq!(stdout[3], "x=0, y=0");
}
