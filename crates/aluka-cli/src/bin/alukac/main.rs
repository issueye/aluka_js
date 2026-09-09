//! alukac 独立前端编译器命令行工具。
//!
//! 支持将 JavaScript / TypeScript 源码编译为标准 ALUKABC1（Version 30）二进制字节码，
//! 以及对 `.bc` 字节码文件进行详细的反汇编检查。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aluka_bytecode::{BytecodeModule, Constant, Op, OperandKind};
use aluka_compiler::{compile_source_unit, optimize_ast};
use aluka_parser::source_unit::{LanguageRegistry, ModuleKind};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// CLI 运行配置选项
#[derive(Debug, PartialEq)]
enum SubCommand {
    Compile {
        input: PathBuf,
        output: Option<PathBuf>,
        optimize: bool,
        /// 产出格式：alukabc1（默认，标准字节码）或 aluc（发布容器）
        format: String,
        /// 剥离调试段（仅 aluc 格式生效）
        strip_debug: bool,
    },
    Disasm {
        input: PathBuf,
    },
    /// 依赖图遍历构建：入口 + require 闭包编译为镜像 .bc 树（express 级
    /// node_modules 依赖链路；.json 原样拷贝）
    Build {
        input: PathBuf,
        /// 输出目录（默认 `aluka_build`）
        output: Option<PathBuf>,
        optimize: bool,
    },
    Version,
    Help,
}

fn main() -> ExitCode {
    const STACK_SIZE: usize = 8 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(run)
        .expect("启动编译器主线程失败")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&args) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("错误: {err}");
            eprintln!("运行 `alukac --help` 查看使用帮助");
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        SubCommand::Version => {
            println!("alukac {VERSION} (Aluka 前端字节码编译器)");
            ExitCode::SUCCESS
        }
        SubCommand::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        SubCommand::Compile {
            input,
            output,
            optimize,
            format,
            strip_debug,
        } => run_compile(&input, output.as_deref(), optimize, &format, strip_debug),
        SubCommand::Disasm { input } => run_disasm(&input),
        SubCommand::Build {
            input,
            output,
            optimize,
        } => aluka_compiler::build::run_build(&input, output.as_deref(), optimize),
    }
}

fn print_usage() {
    println!("alukac {VERSION} - Aluka 字节码编译器与反汇编工具");
    println!();
    println!("用法:");
    println!("  alukac compile <源文件> [-o <输出文件>] [--no-opt]");
    println!("  alukac <源文件> [-o <输出文件>]");
    println!("  alukac disasm <字节码文件>");
    println!("  alukac --disasm <字节码文件>");
    println!("  alukac --version / -v");
    println!("  alukac --help / -h");
    println!();
    println!("选项:");
    println!("  -o <path>       指定输出文件路径（默认替换源文件后缀为 .bc）");
    println!("  --no-opt        关闭编译期静态优化 Pass（常量折叠、死代码消除等）");
    println!("  --disasm        反汇编并格式化打印指定的 .bc 字节码文件");
    println!("  -v, --version   打印版本信息");
    println!("  -h, --help      打印使用帮助");
}

fn parse_args(args: &[String]) -> Result<SubCommand, String> {
    if args.is_empty() {
        return Ok(SubCommand::Help);
    }

    match args[0].as_str() {
        "-v" | "--version" => Ok(SubCommand::Version),
        "-h" | "--help" => Ok(SubCommand::Help),
        "--disasm" => {
            if args.len() < 2 {
                return Err("请指定需要反汇编的 .bc 文件路径".to_owned());
            }
            Ok(SubCommand::Disasm {
                input: PathBuf::from(&args[1]),
            })
        }
        "disasm" => {
            if args.len() < 2 {
                return Err("请指定需要反汇编的 .bc 文件路径".to_owned());
            }
            Ok(SubCommand::Disasm {
                input: PathBuf::from(&args[1]),
            })
        }
        "compile" => {
            if args.len() < 2 {
                return Err("请指定需要编译的源文件路径".to_owned());
            }
            parse_compile_flags(&args[1], &args[2..])
        }
        "build" => {
            if args.len() < 2 {
                return Err("请指定需要构建的入口源文件路径".to_owned());
            }
            parse_build_flags(&args[1], &args[2..])
        }
        other => {
            if other.starts_with('-') {
                return Err(format!("未知选项: {other}"));
            }
            parse_compile_flags(other, &args[1..])
        }
    }
}

fn parse_build_flags(input_path: &str, flags: &[String]) -> Result<SubCommand, String> {
    let mut output = None;
    let mut optimize = true;
    let mut idx = 0;
    while idx < flags.len() {
        match flags[idx].as_str() {
            "-o" => {
                idx += 1;
                if idx >= flags.len() {
                    return Err("-o 选项后缺少输出目录".to_owned());
                }
                output = Some(PathBuf::from(&flags[idx]));
            }
            "--no-opt" => optimize = false,
            other => return Err(format!("build 命令无法识别的参数: {other}")),
        }
        idx += 1;
    }
    Ok(SubCommand::Build {
        input: PathBuf::from(input_path),
        output,
        optimize,
    })
}

fn parse_compile_flags(input_path: &str, flags: &[String]) -> Result<SubCommand, String> {
    let mut output = None;
    let mut optimize = true;
    let mut format: Option<String> = None;
    let mut strip_debug = false;
    let mut idx = 0;

    while idx < flags.len() {
        match flags[idx].as_str() {
            "-o" => {
                idx += 1;
                if idx >= flags.len() {
                    return Err("-o 选项后缺少目标文件路径".to_owned());
                }
                output = Some(PathBuf::from(&flags[idx]));
            }
            "--no-opt" => {
                optimize = false;
            }
            "--format" => {
                idx += 1;
                if idx >= flags.len() {
                    return Err("--format 选项后缺少格式（alukabc1 | aluc）".to_owned());
                }
                format = Some(flags[idx].clone());
            }
            "--strip-debug" => {
                strip_debug = true;
            }
            other => {
                return Err(format!("compile 命令无法识别的参数: {other}"));
            }
        }
        idx += 1;
    }

    Ok(SubCommand::Compile {
        input: PathBuf::from(input_path),
        output,
        optimize,
        format: format.unwrap_or_else(|| "alukabc1".to_owned()),
        strip_debug,
    })
}

/// 执行源文件编译并输出 .bc
fn run_compile(
    input: &Path,
    output: Option<&Path>,
    optimize: bool,
    format: &str,
    strip_debug: bool,
) -> ExitCode {
    let path_str = input.to_string_lossy();
    // 按扩展名推断模块种类：.mjs/.mts → ESM、.cjs/.cts → CommonJS、其余 → Script
    let module_kind = match input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mjs" | "mts") => ModuleKind::Esm,
        Some("cjs" | "cts") => ModuleKind::CommonJs,
        _ => ModuleKind::Script,
    };
    let mut unit = match LanguageRegistry::global().parse_file(&path_str, module_kind) {
        Ok(u) => u,
        Err(aluka_parser::source_unit::SourceUnitError::ReadError { message, .. }) => {
            eprintln!("错误: 无法读取源文件 \"{}\": {message}", input.display());
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("错误: 解析源文件 \"{}\" 失败: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };

    // 1. 静态优化 Pass（若为含 JS AST 单元且开启优化）
    if optimize {
        if let Some(prog) = &mut unit.program {
            optimize_ast(prog);
        }
    }

    // 2. 源码单元编译与阶段位追踪推进
    let module = match compile_source_unit(&mut unit) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("编译源文件失败 \"{}\": {e}", input.display());
            return ExitCode::FAILURE;
        }
    };

    // 4. 静态 ISA 规范校验
    if let Err(e) = module.verify() {
        eprintln!("字节码校验未通过: {e}");
        return ExitCode::FAILURE;
    }

    // 5. 序列化：alukabc1（标准字节码，默认）或 aluc（发布容器）
    let bytes = match format {
        "aluc" => module.serialize_aluc(strip_debug),
        "alukabc1" => module.serialize(),
        other => {
            eprintln!("错误: 未知格式 \"{other}\"（可选 alukabc1 | aluc）");
            return ExitCode::FAILURE;
        }
    };

    // 6. 写出到目标文件
    let target_path = match output {
        Some(p) => p.to_path_buf(),
        None => input.with_extension("bc"),
    };

    if let Err(e) = std::fs::write(&target_path, &bytes) {
        eprintln!("写入字节码文件失败 \"{}\": {e}", target_path.display());
        return ExitCode::FAILURE;
    }

    println!(
        "编译成功: {} -> {} ({} 字节, 函数: {}, 类: {})",
        input.display(),
        target_path.display(),
        bytes.len(),
        module.functions.len(),
        module.classes.len()
    );

    ExitCode::SUCCESS
}

/// 执行反汇编检查
fn run_disasm(input: &Path) -> ExitCode {
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("错误: 无法读取字节码文件 \"{}\": {e}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let module = match BytecodeModule::deserialize(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("错误: 字节码反序列化失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("============================================================");
    println!("Aluka 字节码反汇编报告");
    println!("============================================================");
    println!("文件路径:   {}", input.display());
    println!("文件大小:   {} 字节", bytes.len());
    println!("格式魔数:   ALUKABC1");
    println!("规范版本:   30");
    println!("函数模板数: {}", module.functions.len());
    println!("类模板数:   {}", module.classes.len());
    println!("============================================================");

    for (f_idx, func) in module.functions.iter().enumerate() {
        println!();
        println!(
            "--- 函数模板 #{f_idx}: \"{}\" (源文件: {}) ---",
            func.name, func.source_file
        );
        println!(
            "属性: 参数量: {}, 局部槽位: {}, 最大栈深: {}, 箭头函数: {}, 生成器: {}, 异步: {}",
            func.num_params,
            func.num_locals,
            func.max_stack,
            func.is_arrow,
            func.is_generator,
            func.is_async
        );

        if !func.constants.is_empty() {
            println!("常量池 ({} 项):", func.constants.len());
            for (c_idx, c) in func.constants.iter().enumerate() {
                println!("  #{c_idx:<3} = {}", format_constant(c));
            }
        }

        if !func.upvalues.is_empty() {
            println!("上值捕获表 ({} 项):", func.upvalues.len());
            for (u_idx, up) in func.upvalues.iter().enumerate() {
                println!(
                    "  [u{u_idx}] is_local: {}, slot_or_idx: {}",
                    up.is_local, up.index
                );
            }
        }

        if !func.try_table.is_empty() {
            println!("异常保护表 ({} 项):", func.try_table.len());
            for (t_idx, entry) in func.try_table.iter().enumerate() {
                println!(
                    "  [try{t_idx}] 保护区: 0x{:04x}..0x{:04x}, Catch: 0x{:04x}..0x{:04x}, Finally: 0x{:04x}..0x{:04x}",
                    entry.start_pc,
                    entry.end_pc,
                    entry.catch_pc,
                    entry.catch_end_pc,
                    entry.finally_pc,
                    entry.finally_end_pc
                );
            }
        }

        println!("指令序列 ({} 条):", func.code.len());
        for (pc_idx, instr) in func.code.iter().enumerate() {
            let byte_off = pc_idx * 4;
            let mnemonic = instr.op.name();
            let op_str = format_operand(instr.op, instr.operand, pc_idx, &func.constants);
            println!("  {byte_off:04x}:  {mnemonic:<18} {op_str}");
        }
    }

    if !module.classes.iter().all(|c| c.name.is_empty()) {
        println!();
        println!("============================================================");
        println!("类模板定义列表");
        println!("============================================================");
        for (c_idx, class) in module.classes.iter().enumerate() {
            println!("--- 类模板 #{c_idx}: \"{}\" ---", class.name);
            println!("构造函数索引: #{}", class.constructor_index);
            println!("继承父类:     {}", class.has_super);
            println!("方法列表 ({} 项):", class.methods.len());
            for method in &class.methods {
                println!(
                    "  - \"{}\" -> 函数模板 #{} (静态: {})",
                    method.name, method.func_index, method.is_static
                );
            }
        }
    }

    ExitCode::SUCCESS
}

fn format_constant(c: &Constant) -> String {
    match c {
        Constant::Number(v) => format!("Number({v})"),
        Constant::String(s) => format!("String(\"{s}\")"),
        Constant::BigInt(b) => format!("BigInt({b})"),
        Constant::Bool(b) => format!("Bool({b})"),
        Constant::Null => "Null".to_owned(),
    }
}

fn format_operand(op: Op, operand: u32, current_pc: usize, constants: &[Constant]) -> String {
    match op.operand_kind() {
        OperandKind::None => String::new(),
        OperandKind::ConstIdx => {
            let idx = operand as usize;
            if idx < constants.len() {
                format!("#{operand} ({})", format_constant(&constants[idx]))
            } else {
                format!("#{operand}")
            }
        }
        OperandKind::Slot => format!("slot {operand}"),
        OperandKind::Int => format!("{operand}"),
        OperandKind::UpvalueIdx => format!("u{operand}"),
        OperandKind::TemplateIdx => format!("tpl #{operand}"),
        OperandKind::TryIdx => format!("try #{operand}"),
        OperandKind::Count => format!("count {operand}"),
        OperandKind::SignedOff => {
            // 24 位有符号相对偏移（与 VM compute_jump_target 同款符号扩展）
            let signed = if operand & 0x80_0000 != 0 {
                (operand | 0xFF00_0000) as i32
            } else {
                operand as i32
            };
            let off = signed / 4;
            let target_pc = (current_pc as i32 + 1 + off) * 4;
            format!("{off:+} -> 0x{target_pc:04x}")
        }
        OperandKind::PackedSlotName => {
            let slot = operand >> 16;
            let name_idx = operand & 0xFFFF;
            format!("slot {slot}, name #{name_idx}")
        }
        OperandKind::PackedCall => {
            let num_args = operand >> 16;
            let name_idx = operand & 0xFFFF;
            format!("args {num_args}, method #{name_idx}")
        }
    }
}
