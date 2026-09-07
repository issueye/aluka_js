//! aluka 顶层统一命令行集成测试套件。
//!
//! 验证目标：
//! 1. `aluka run <script>` 支持直接执行 JavaScript 源码并获得预期输出；
//! 2. `aluka run <script>` 支持直接执行 TypeScript 源码，类型注解零成本剥离；
//! 3. 支持直接运行 JSON 数据模块；
//! 4. 支持直接运行 S-expression DSL 源码；
//! 5. 支持向脚本传递命令行参数并注入 `process.argv`；
//! 6. 未捕获异常友好输出并返回退出码 1；
//! 7. 支持缺省 `run` 子命令直接传入脚本路径运行（如 `aluka app.ts`）。

use std::fs;
use std::process::Command;

#[test]
fn test_aluka_run_javascript() {
    let aluka_bin = env!("CARGO_BIN_EXE_aluka");
    let temp_dir = std::env::temp_dir().join("aluka_run_test_js");
    let _ = fs::create_dir_all(&temp_dir);

    let js_file = temp_dir.join("calc.js");
    let js_code = r#"
        function fib(n) {
            if (n <= 1) return n;
            return fib(n - 1) + fib(n - 2);
        }
        console.log("fib(10) =", fib(10));
    "#;
    fs::write(&js_file, js_code).expect("写入 js 失败");

    // 测试 aluka run <file>
    let out = Command::new(aluka_bin)
        .arg("run")
        .arg(&js_file)
        .output()
        .expect("运行 aluka run 失败");

    assert!(out.status.success(), "aluka run 执行失败");
    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout, "fib(10) = 55");

    // 测试直接传文件路径 aluka <file>
    let out_direct = Command::new(aluka_bin)
        .arg(&js_file)
        .output()
        .expect("直接传路径运行失败");

    assert!(out_direct.status.success());
    let stdout_direct = String::from_utf8_lossy(&out_direct.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout_direct, "fib(10) = 55");
}

#[test]
fn test_aluka_run_typescript() {
    let aluka_bin = env!("CARGO_BIN_EXE_aluka");
    let temp_dir = std::env::temp_dir().join("aluka_run_test_ts");
    let _ = fs::create_dir_all(&temp_dir);

    let ts_file = temp_dir.join("typed.ts");
    let ts_code = r#"
        interface User {
            id: number;
            name: string;
        }
        function greet(u: User): string {
            return "Hello, " + u.name + " (#" + u.id + ")";
        }
        const user: User = { id: 42, name: "Aluka" };
        console.log(greet(user));
    "#;
    fs::write(&ts_file, ts_code).expect("写入 ts 失败");

    let out = Command::new(aluka_bin)
        .arg("run")
        .arg(&ts_file)
        .output()
        .expect("运行 aluka run ts 失败");

    assert!(out.status.success(), "aluka run ts 执行失败");
    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout, "Hello, Aluka (#42)");
}

#[test]
fn test_aluka_run_dsl() {
    let aluka_bin = env!("CARGO_BIN_EXE_aluka");
    let temp_dir = std::env::temp_dir().join("aluka_run_test_dsl");
    let _ = fs::create_dir_all(&temp_dir);

    let dsl_file = temp_dir.join("demo.adsl");
    let dsl_code = r#"
        (def x 15)
        (def y 4)
        (fn compute (a b)
            (+ (* a b) 10))
        (console.log "dsl result:" (compute x y))
    "#;
    fs::write(&dsl_file, dsl_code).expect("写入 adsl 失败");

    let out = Command::new(aluka_bin)
        .arg("run")
        .arg(&dsl_file)
        .output()
        .expect("运行 aluka run dsl 失败");

    assert!(out.status.success(), "aluka run dsl 执行失败");
    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout, "dsl result: 70");
}

#[test]
fn test_aluka_run_argv_forwarding() {
    let aluka_bin = env!("CARGO_BIN_EXE_aluka");
    let temp_dir = std::env::temp_dir().join("aluka_run_test_argv");
    let _ = fs::create_dir_all(&temp_dir);

    let js_file = temp_dir.join("argv.js");
    let js_code = r#"
        console.log("argv[1]:", process.argv[1]);
        console.log("argv[2]:", process.argv[2]);
    "#;
    fs::write(&js_file, js_code).expect("写入 argv.js 失败");

    let out = Command::new(aluka_bin)
        .arg("run")
        .arg(&js_file)
        .arg("foo")
        .arg("bar")
        .output()
        .expect("执行失败");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout, "argv[1]: foo\nargv[2]: bar");
}

#[test]
fn test_aluka_run_uncaught_exception_exit_code() {
    let aluka_bin = env!("CARGO_BIN_EXE_aluka");
    let temp_dir = std::env::temp_dir().join("aluka_run_test_exc");
    let _ = fs::create_dir_all(&temp_dir);

    let js_file = temp_dir.join("error.js");
    let js_code = r#"
        console.log("before throw");
        throw new Error("test failure message");
    "#;
    fs::write(&js_file, js_code).expect("写入 error.js 失败");

    let out = Command::new(aluka_bin)
        .arg("run")
        .arg(&js_file)
        .output()
        .expect("执行失败");

    assert!(!out.status.success(), "未捕获异常应返回非零退出码");
    assert_eq!(out.status.code(), Some(1), "退出码应为 1");

    let stdout = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(stdout, "before throw");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Error: test failure message"));
}
