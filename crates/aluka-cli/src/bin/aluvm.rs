//! aluvm 独立虚拟机命令行工具。
//!
//! 执行 ALUKABC1（Version 30）字节码模块：加载 → Verifier 校验 → Tier 0 解释执行。
//! 未捕获异常输出到 stderr 并以非零退出码结束；`process.argv` 按脚本路径 + 参数注入。
//!
//! 执行装配（容器嗅探/校验/eval provider/worker 钩子/退出码映射）在
//! `aluka_runtime::execute_bc`——与统一 `aluka` 单二进制共用同一流程
//! （M7.1：单文件分发形态整合，内部流水线不变）。

use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// CLI 运行配置选项
#[derive(Debug, PartialEq)]
enum SubCommand {
    Run { input: PathBuf, args: Vec<String> },
    Version,
    Help,
}

fn main() -> ExitCode {
    // JS 递归深度受解释器原生栈约束：主线程默认栈（Windows 1MB）在 fib(20)
    // 即溢出。改在专用大栈线程执行（虚拟内存按需提交，预留不等于占用）。
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(real_main)
        .expect("spawn vm thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn real_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&args) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("错误: {err}");
            eprintln!("运行 `aluvm --help` 查看使用帮助");
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        SubCommand::Version => {
            println!("aluvm {VERSION} (Aluka Tier 0 字节码虚拟机)");
            ExitCode::SUCCESS
        }
        SubCommand::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        SubCommand::Run { input, args } => aluka_runtime::execute_bc(&input, &args),
    }
}

fn print_usage() {
    println!("aluvm {VERSION} - Aluka 字节码虚拟机");
    println!();
    println!("用法:");
    println!("  aluvm run <字节码文件> [参数...]");
    println!("  aluvm <字节码文件> [参数...]");
    println!("  aluvm --version / -v");
    println!("  aluvm --help / -h");
    println!();
    println!("说明:");
    println!("  执行前经 Verifier 严格校验（V1..V16），拒绝未通过的字节码；");
    println!("  参数经 `process.argv` 注入（argv[0]=脚本路径，其后为命令行参数）；");
    println!("  未捕获异常打印到 stderr，进程退出码为 1。");
    println!("  提示：源码请用 alukac 编译为 .bc，或直接使用 `aluka run`（自动编译）。");
}

fn parse_args(args: &[String]) -> Result<SubCommand, String> {
    if args.is_empty() {
        return Ok(SubCommand::Help);
    }

    match args[0].as_str() {
        "-v" | "--version" => Ok(SubCommand::Version),
        "-h" | "--help" => Ok(SubCommand::Help),
        "run" => {
            if args.len() < 2 {
                return Err("run 需要指定 .bc 字节码文件路径".to_owned());
            }
            Ok(SubCommand::Run {
                input: PathBuf::from(&args[1]),
                args: args[2..].to_vec(),
            })
        }
        other => {
            if other.starts_with('-') {
                return Err(format!("未知选项: {other}"));
            }
            Ok(SubCommand::Run {
                input: PathBuf::from(other),
                args: args[1..].to_vec(),
            })
        }
    }
}
