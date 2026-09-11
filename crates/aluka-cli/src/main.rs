//! aluka 统一顶层运行时命令行工具。
//!
//! 支持直接运行 JavaScript、TypeScript、JSON 数据模块以及 DSL 脚本源码，
//! 内部集成解析、TS 类型剥离、字节码编译、Verifier 静态校验与虚拟机执行一体化流水线。
//!
//! M7.1 单二进制形态：`run`（自动编译/校验/构建镜像后执行）、`build`
//! （依赖闭包镜像构建）、`npm`（包管理器分发）、`test`（node:test 用例
//! 运行器，M5.4 切片一）。**内部分层流水线不变**：源码 → 编译/静态校验 →
//! VM 解释执行。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aluka_runtime::{ReporterKind, Runtime};

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
    /// `node:test` 用例运行器：目标为用例文件或目录（空 = 从 cwd 发现）。
    Test {
        targets: Vec<PathBuf>,
        reporter: ReporterKind,
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
        Command::Test {
            targets,
            reporter,
            optimize,
        } => test_command(&targets, reporter, optimize),
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
    println!("  aluka test [--test-reporter=<spec|tap|dot|lcov>] [<文件|目录>...]");
    println!("  aluka --capabilities");
    println!("  aluka -v, --version");
    println!("  aluka -h, --help");
    println!();
    println!("选项与说明:");
    println!("  run <脚本>      执行指定的 .js / .ts / .json / .adsl / .bc 文件；");
    println!("                  源码带 node_modules 依赖时自动构建镜像后执行");
    println!("  build <脚本>    编译 require 依赖闭包为镜像 .bc 树（默认 aluka_build/）");
    println!("  npm ...         包管理器（npm 功能复刻，等价 aluka-npm）");
    println!("  test [目标...]  运行 node:test 用例（无目标时从 cwd 发现）；");
    println!("                  任一用例失败即退出码 1");
    println!("  --test-reporter=<spec|tap|dot|lcov>  test 报告器（默认 spec）");
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
        "test" => parse_test(&args[1..]),
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
            // `process.exit(code)`：宿主按该退出码终止（Node 语义）
            match runtime.exit_code() {
                Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
                None => ExitCode::SUCCESS,
            }
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

/// `test` 参数解析：目标（文件/目录）+ `--test-reporter=<spec|tap|dot>` + `--no-opt`。
fn parse_test(args: &[String]) -> Result<Command, String> {
    let mut targets = Vec::new();
    let mut reporter = ReporterKind::Spec;
    let mut optimize = true;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--no-opt" => optimize = false,
            "--test-reporter" => {
                idx += 1;
                if idx >= args.len() {
                    return Err("--test-reporter 后缺少报告器名称".to_owned());
                }
                reporter = parse_reporter(&args[idx])?;
            }
            other if other.starts_with("--test-reporter=") => {
                reporter = parse_reporter(other.trim_start_matches("--test-reporter="))?;
            }
            other if other.starts_with('-') => {
                return Err(format!("test 命令无法识别的参数: {other}"));
            }
            other => targets.push(PathBuf::from(other)),
        }
        idx += 1;
    }
    Ok(Command::Test {
        targets,
        reporter,
        optimize,
    })
}

/// 报告器名称解析（对齐 Node `--test-reporter` 的三个内置形态）。
fn parse_reporter(name: &str) -> Result<ReporterKind, String> {
    match name {
        "spec" => Ok(ReporterKind::Spec),
        "tap" => Ok(ReporterKind::Tap),
        "dot" => Ok(ReporterKind::Dot),
        "lcov" => Ok(ReporterKind::Lcov),
        other => Err(format!(
            "未知报告器: {other}（可用: spec / tap / dot / lcov）"
        )),
    }
}

/// `test` 分发：发现用例文件 → 每文件独立 `Runtime` 执行 → 聚合退出码。
///
/// 对齐 Node「每个测试文件独立运行」的语义（本仓以独立 `Runtime` 实例等价
/// 实现）；任一文件出现失败用例、或该文件执行/编译失败 → 退出码 1。
///
/// 报告格式源自本仓 Go CLI 契约（`printTestLine` + 汇总块），**不声称**与
/// `node --test` 的报告输出逐字一致。
fn test_command(targets: &[PathBuf], reporter: ReporterKind, optimize: bool) -> ExitCode {
    let files = collect_test_files(targets);
    if files.is_empty() {
        eprintln!(
            "未发现任何测试文件（命名约定: *.test.js / *-test.js，或 test/、tests/ 目录下的脚本）"
        );
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for file in &files {
        let mut runtime = Runtime::new();
        runtime.enable_test_runner(reporter);
        match runtime.execute_file(file, &[], optimize) {
            Ok(_) => {
                for line in runtime.stdout_records() {
                    println!("{line}");
                }
                // lcov：覆盖率报告直出（LCOV tracefile 文本）
                if reporter == ReporterKind::Lcov
                    && let Some(report) = runtime.lcov_report()
                {
                    print!("{report}");
                }
                // 测试文件内显式 `process.exit(code)`：非零即视为失败
                if runtime.exit_code().is_some_and(|code| code != 0) {
                    failed = true;
                }
                if runtime.test_summary().is_some_and(|c| c.fail > 0) {
                    failed = true;
                }
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
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// 收集用例文件：显式文件直接采纳；目录按 Node 约定递归发现；无目标时从 cwd 发现。
fn collect_test_files(targets: &[PathBuf]) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = if targets.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        targets.to_vec()
    };
    let mut out = Vec::new();
    for root in roots {
        if root.is_file() {
            // 显式指定的文件一律采纳（不套命名约定）
            out.push(root);
        } else if root.is_dir() {
            walk_test_files(&root, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 递归发现用例文件：忽略 `node_modules`；`test/`、`tests/` 目录下取全部脚本，
/// 其余目录只取命名符合约定者。
fn walk_test_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let in_test_dir = dir.file_name().is_some_and(|n| n == "test" || n == "tests");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            walk_test_files(&path, out);
            continue;
        }
        if !is_script_file(&path) {
            continue;
        }
        if in_test_dir || is_test_file_name(&path) {
            out.push(path);
        }
    }
}

/// 是否脚本文件（`.js` / `.cjs` / `.mjs` / `.ts`）。
fn is_script_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js") | Some("cjs") | Some("mjs") | Some("ts")
    )
}

/// 文件名是否符合用例命名约定（`*.test.js` / `*-test.js`）。
fn is_test_file_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.ends_with(".test") || stem.ends_with("-test"))
}
