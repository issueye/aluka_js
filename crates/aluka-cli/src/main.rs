//! aluka 统一顶层运行时命令行工具。
//!
//! 支持直接运行 JavaScript、TypeScript、JSON 数据模块以及 DSL 脚本源码，
//! 内部集成解析、TS 类型剥离、字节码编译、Verifier 静态校验与虚拟机执行一体化流水线。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aluka_runtime::Runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, PartialEq)]
enum Command {
    Run {
        script: PathBuf,
        args: Vec<String>,
        optimize: bool,
    },
    Capabilities,
    Version,
    Help,
}

fn main() -> ExitCode {
    // 同 aluvm：解释执行放专用大栈线程，避免 JS 深递归触发原生栈溢出。
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(real_main)
        .expect("spawn vm thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn real_main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&raw_args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("错误: {msg}");
            eprintln!("运行 `aluka --help` 查看使用说明");
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        Command::Version => {
            println!("aluka {VERSION} (JavaScript/TypeScript 现代运行时)");
            ExitCode::SUCCESS
        }
        Command::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        Command::Capabilities => {
            print_capabilities();
            ExitCode::SUCCESS
        }
        Command::Run {
            script,
            args,
            optimize,
        } => run_script(&script, &args, optimize),
    }
}

fn print_usage() {
    println!("aluka {VERSION} - JavaScript / TypeScript 统一运行时引擎");
    println!();
    println!("用法:");
    println!("  aluka run <脚本文件> [参数...] [--no-opt]");
    println!("  aluka <脚本文件> [参数...] [--no-opt]");
    println!("  aluka --capabilities");
    println!("  aluka -v, --version");
    println!("  aluka -h, --help");
    println!();
    println!("选项与说明:");
    println!("  run <脚本>      执行指定的 .js / .ts / .json / .adsl 脚本");
    println!("  --no-opt        关闭编译期静态优化 Pass");
    println!("  --capabilities  查看当前引擎已装配能力域与内置模块迁移进度");
    println!("  -v, --version   打印版本信息");
    println!("  -h, --help      打印使用帮助");
    println!();
    println!("示例:");
    println!("  aluka run app.ts arg1 arg2");
    println!("  aluka main.js");
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::Help);
    }

    match args[0].as_str() {
        "-v" | "--version" => Ok(Command::Version),
        "-h" | "--help" => Ok(Command::Help),
        "--capabilities" => Ok(Command::Capabilities),
        "run" => {
            if args.len() < 2 {
                return Err("`run` 命令需要指定目标脚本文件路径".to_owned());
            }
            let script = PathBuf::from(&args[1]);
            let mut script_args = Vec::new();
            let mut optimize = true;

            for arg in &args[2..] {
                if arg == "--no-opt" {
                    optimize = false;
                } else {
                    script_args.push(arg.clone());
                }
            }

            Ok(Command::Run {
                script,
                args: script_args,
                optimize,
            })
        }
        other => {
            if other.starts_with('-') {
                return Err(format!("未知选项: {other}"));
            }
            let script = PathBuf::from(other);
            let mut script_args = Vec::new();
            let mut optimize = true;

            for arg in &args[1..] {
                if arg == "--no-opt" {
                    optimize = false;
                } else {
                    script_args.push(arg.clone());
                }
            }

            Ok(Command::Run {
                script,
                args: script_args,
                optimize,
            })
        }
    }
}

fn run_script(script: &Path, args: &[String], optimize: bool) -> ExitCode {
    let mut runtime = Runtime::new();
    match runtime.execute_file(script, args, optimize) {
        Ok(_) => {
            for line in runtime.stdout_records() {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            for line in runtime.stdout_records() {
                println!("{line}");
            }
            if let Some(msg) = runtime.uncaught_formatted() {
                eprintln!("{msg}");
            } else {
                eprintln!("{err}");
            }
            ExitCode::FAILURE
        }
    }
}

fn print_capabilities() {
    use aluka_builtins::ModuleStatus;

    let runtime = Runtime::new();
    println!("capabilities ({}):", runtime.capabilities().len());
    for capability in runtime.capabilities() {
        let deps = capability.dependencies();
        if deps.is_empty() {
            println!("  {capability:?}");
        } else {
            println!("  {capability:?} <- {deps:?}");
        }
    }

    let builtins = runtime.builtins();
    println!();
    println!("builtin modules: {} registered", builtins.len());
    println!("  native:  {}", builtins.count(ModuleStatus::Native));
    println!("  bridged: {}", builtins.count(ModuleStatus::ForeignBridge));
    println!("  planned: {}", builtins.count(ModuleStatus::Planned));
}
