//! aluvm 独立虚拟机命令行工具。
//!
//! 执行 ALUKABC1（Version 30）字节码模块：加载 → Verifier 校验 → Tier 0 解释执行。
//! 未捕获异常输出到 stderr 并以非零退出码结束；`process.argv` 按脚本路径 + 参数注入。

use std::path::PathBuf;
use std::process::ExitCode;

use aluka_bytecode::BytecodeModule;
use aluka_parser::source_unit::{LanguageRegistry, ModuleKind};
use aluka_vm::{Value, Vm};

/// 装配动态求值编译器 Hook：源码 → 编译 → 字节码模块。
/// （eval / new Function 经此在运行时按需编译，动态产物仍强制 Verifier 校验）
fn install_eval_provider(vm: &mut Vm) {
    vm.set_eval_provider(|src: &str| {
        // 空源码：求值结果为 undefined（规范），无需编译
        if src.trim().is_empty() {
            return Ok(aluka_vm::empty_eval_module());
        }
        let mut unit = LanguageRegistry::global()
            .parse_source(src, "<eval>", ModuleKind::Script)
            .map_err(|e| e.to_string())?;
        let Some(program) = unit.program.take() else {
            return Err("unexpected end of input".to_owned());
        };
        // eval 以脚本完成值语义求值：完整编译管线 + 保留末语句值开关
        let mut compiler = aluka_compiler::ModuleCompiler {
            preserve_completion_value: true,
            implicit_globals: true,
            ..Default::default()
        };
        let module = compiler.compile(&program);
        Ok(module)
    });
}

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
        SubCommand::Run { input, args } => run_bc(&input, &args),
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
    println!("  提示：源码请先用 alukac 编译为 .bc 字节码。");
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

/// 执行字节码模块：加载、校验、运行、按退出码映射收尾。
fn run_bc(input: &std::path::Path, cli_args: &[String]) -> ExitCode {
    let data = match std::fs::read(input) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("错误: 无法读取 {}: {err}", input.display());
            return ExitCode::FAILURE;
        }
    };
    // 按魔数嗅探 ALUKACC1（发布容器）或 ALUKABC1（标准字节码格式）
    let (module, payload_range) = match BytecodeModule::load_any_container(&data) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("错误: 反序列化 {} 失败: {err}", input.display());
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = module.verify() {
        eprintln!("错误: {} 未通过 Verifier 校验: {err}", input.display());
        return ExitCode::FAILURE;
    }

    let mut vm = Vm::new(0);
    install_eval_provider(&mut vm);
    inject_process_argv(&mut vm, input, cli_args);
    vm.setup_cjs(input); // CJS 模块上下文（require/exports/循环依赖）
    // 函数扩展标量头（arguments 槽位等）
    if let Err(err) = vm.load_module(&data[payload_range], &module) {
        eprintln!("错误: functions 标量头不完整: {err}");
        return ExitCode::FAILURE;
    }

    match vm.run_module(&module) {
        Ok(_) => {
            for line in &vm.stdout_records {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            for line in &vm.stdout_records {
                println!("{line}");
            }
            match err {
                aluka_vm::VmError::Thrown(exc) => {
                    eprintln!("{}", format_uncaught(&mut vm, exc));
                }
                other => {
                    eprintln!("虚拟机内部错误: {other}");
                }
            }
            eprintln!("    at <module> ({})", input.display());
            ExitCode::FAILURE
        }
    }
}

/// 把脚本路径与命令行参数注入 `process.argv`（argv[0]=脚本路径，对齐 Node 语义的脚本段）。
fn inject_process_argv(vm: &mut Vm, input: &std::path::Path, cli_args: &[String]) {
    // argv 注入到 VM 的 process 单例（interpreter 的 nextTick 等拦截按单例匹配）
    let mut argv = vec![Value::Object(vm.alloc_string(input.display().to_string()))];
    for arg in cli_args {
        argv.push(Value::Object(vm.alloc_string(arg.clone())));
    }
    let argv_arr = Value::Object(vm.alloc_array(argv));
    if let Some(p) = vm.process_object {
        let _ = vm.set_property(Value::Object(p), "argv", argv_arr);
    }
}

/// 未捕获异常的友好展示：Error 实例输出 `Name: message`，其余值原样格式化。
fn format_uncaught(vm: &mut Vm, exc: Value) -> String {
    // 调试定位：ALUKA_ERR_TRACE=1 时附带出错函数与指令下标
    if std::env::var("ALUKA_ERR_TRACE").is_ok() {
        let fname = vm
            .module_functions
            .get(vm.current_func_idx.max(0) as usize)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "?".to_owned());
        eprintln!(
            "[err-trace] func_idx={} func={fname} last_pc={}",
            vm.current_func_idx, vm.last_pc
        );
    }
    if matches!(exc, Value::Object(_)) {
        let name = vm
            .get_property(exc, "name")
            .ok()
            .map(|v| vm.format_value(v))
            .unwrap_or_default();
        let message = vm
            .get_property(exc, "message")
            .ok()
            .map(|v| vm.format_value(v))
            .unwrap_or_default();
        if !name.is_empty() && name != "undefined" {
            return format!("{name}: {message}");
        }
    }
    vm.format_value(exc)
}
