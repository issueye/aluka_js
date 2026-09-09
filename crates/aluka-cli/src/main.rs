//! aluka 统一顶层运行时命令行工具。
//!
//! 支持直接运行 JavaScript、TypeScript、JSON 数据模块以及 DSL 脚本源码，
//! 内部集成解析、TS 类型剥离、字节码编译、Verifier 静态校验与虚拟机执行一体化流水线。
//!
//! M7.1 单二进制形态：`run`（自动编译/校验/构建镜像后执行）、`build`
//! （依赖闭包镜像构建）、`npm`（包管理器分发）。**内部分层流水线不变**：
//! 源码 → 编译/静态校验 → VM 解释执行。

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
    Build {
        script: PathBuf,
        output: Option<PathBuf>,
        optimize: bool,
    },
    Npm {
        args: Vec<String>,
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
        } => run_command(&script, &args, optimize),
        Command::Build {
            script,
            output,
            optimize,
        } => aluka_compiler::build::run_build(&script, output.as_deref(), optimize),
        Command::Npm { args } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match aluka_npm::commands::dispatch(&args, &cwd) {
                Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
                Err(e) => {
                    eprintln!("aluka-npm 错误: {}", e.message);
                    ExitCode::from(e.code.clamp(0, 255) as u8)
                }
            }
        }
    }
}

fn print_usage() {
    println!("aluka {VERSION} - JavaScript / TypeScript 统一运行时引擎");
    println!();
    println!("用法:");
    println!("  aluka run <脚本文件> [参数...] [--no-opt]");
    println!("  aluka <脚本文件> [参数...] [--no-opt]");
    println!("  aluka build <脚本文件> [-o 输出目录] [--no-opt]");
    println!("  aluka npm <install|uninstall|run|ls|init|view> [参数...]");
    println!("  aluka --capabilities");
    println!("  aluka -v, --version");
    println!("  aluka -h, --help");
    println!();
    println!("选项与说明:");
    println!("  run <脚本>      执行指定的 .js / .ts / .json / .adsl / .bc 文件；");
    println!("                  源码带 node_modules 依赖时自动构建镜像后执行");
    println!("  build <脚本>    编译 require 依赖闭包为镜像 .bc 树（默认 aluka_build/）");
    println!("  npm ...         包管理器（npm 功能复刻，等价 aluka-npm）");
    println!("  --no-opt        关闭编译期静态优化 Pass");
    println!("  --capabilities  查看当前引擎已装配能力域与内置模块迁移进度");
    println!("  -v, --version   打印版本信息");
    println!("  -h, --help      打印使用帮助");
    println!();
    println!("示例:");
    println!("  aluka run app.ts arg1 arg2");
    println!("  aluka npm install express && aluka run server.js");
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
        "npm" => Ok(Command::Npm {
            args: args[1..].to_vec(),
        }),
        "build" => {
            if args.len() < 2 {
                return Err("`build` 命令需要指定目标脚本文件路径".to_owned());
            }
            let script = PathBuf::from(&args[1]);
            let mut output = None;
            let mut optimize = true;
            let mut idx = 2;
            while idx < args.len() {
                match args[idx].as_str() {
                    "-o" => {
                        idx += 1;
                        if idx >= args.len() {
                            return Err("-o 选项后缺少输出目录".to_owned());
                        }
                        output = Some(PathBuf::from(&args[idx]));
                    }
                    "--no-opt" => optimize = false,
                    other => return Err(format!("build 命令无法识别的参数: {other}")),
                }
                idx += 1;
            }
            Ok(Command::Build {
                script,
                output,
                optimize,
            })
        }
        "run" => parse_run(&args[1..]),
        other => {
            if other.starts_with('-') {
                return Err(format!("未知选项: {other}"));
            }
            // 直接传文件路径形态：`aluka <file> [参数...] [--no-opt]`
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
                script: PathBuf::from(other),
                args: script_args,
                optimize,
            })
        }
    }
}

/// `run` 参数解析（脚本 + 参数 + `--no-opt`）。
fn parse_run(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("`run` 命令需要指定目标脚本文件路径".to_owned());
    }
    let script = PathBuf::from(&args[0]);
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

/// `run` 分发：.bc 直执行；源码带 node_modules 依赖 → 自动构建镜像执行；
/// 纯源码 → 原有单文件编译执行（流水线不变）。
fn run_command(script: &Path, args: &[String], optimize: bool) -> ExitCode {
    if script.extension().and_then(|e| e.to_str()) == Some("bc") {
        return aluka_runtime::execute_bc(script, args);
    }
    // 依赖判定：项目根（最近 package.json 所在目录）存在 node_modules
    // → 全树镜像构建后执行。项目根以外不外溢（祖先链止于 package.json）。
    let project_root =
        find_project_root(&script.parent().map(Path::to_path_buf).unwrap_or_default());
    let needs_build = project_root
        .as_ref()
        .is_some_and(|root| root.join("node_modules").is_dir());
    if needs_build {
        let root = project_root.expect("needs_build 已保证");
        let outdir = root.join("aluka_build");
        // build 产物：入口源文件扩展名替换为 .bc（`server.js` → `server.bc`）
        let entry_bc = outdir.join(format!(
            "{}.bc",
            script
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        let build = aluka_compiler::build::run_build(script, Some(&outdir), optimize);
        if build != ExitCode::SUCCESS {
            return build;
        }
        return aluka_runtime::execute_bc(&entry_bc, args);
    }
    run_script(script, args, optimize)
}

/// 项目根定位：从目录逐级向上找最近的 `package.json`（npm 语义的项目边界；
/// 越过项目根的 node_modules 不属于本项目）。
fn find_project_root(from: &Path) -> Option<PathBuf> {
    let mut dir = Some(from.to_path_buf());
    while let Some(d) = dir {
        if d.join("package.json").is_file() {
            return Some(d);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
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
