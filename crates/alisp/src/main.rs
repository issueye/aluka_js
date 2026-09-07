//! alisp CLI：`.lisp` 源码 → `.aluc` 发布容器（或 `.alua` 文本汇编）。

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut format = String::from("aluc");
    let mut disasm = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    eprintln!("错误: -o 后缺少目标路径");
                    return ExitCode::FAILURE;
                };
                output = Some(PathBuf::from(p));
            }
            "--format" => {
                i += 1;
                let Some(f) = args.get(i) else {
                    eprintln!("错误: --format 后缺少格式（aluc | alua）");
                    return ExitCode::FAILURE;
                };
                format = f.clone();
            }
            "--disasm" => disasm = true,
            "--help" | "-h" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other => input = Some(PathBuf::from(other)),
        }
        i += 1;
    }
    let Some(input) = input else {
        print_usage();
        return ExitCode::FAILURE;
    };

    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: 无法读取源文件 \"{}\": {e}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let forms = match alisp::parse_program(&src) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("错误: 解析失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut compiler = alisp::Compiler::new();
    let out = match compiler.compile_module(&forms) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("错误: 编译失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    // 静态 ISA 校验（契约内建）
    if let Err(e) = out.module.verify() {
        eprintln!("错误: 字节码校验未通过: {e}");
        return ExitCode::FAILURE;
    }

    let target: PathBuf = output
        .clone()
        .unwrap_or_else(|| input.with_extension("aluc"));
    if disasm || format == "alua" {
        let text = out.module.write_alua();
        let p = if disasm {
            target.with_extension("alua")
        } else {
            target
        };
        if let Err(e) = std::fs::write(&p, text) {
            eprintln!("错误: 写入失败 \"{}\": {e}", p.display());
            return ExitCode::FAILURE;
        }
        println!("汇编输出: {} -> {}", input.display(), p.display());
        return ExitCode::SUCCESS;
    }

    let bytes = out.module.serialize_aluc(false);
    if let Err(e) = std::fs::write(&target, &bytes) {
        eprintln!("错误: 写入失败 \"{}\": {e}", target.display());
        return ExitCode::FAILURE;
    }
    println!(
        "编译成功: {} -> {} ({} 字节, 函数: {})",
        input.display(),
        target.display(),
        bytes.len(),
        out.module.functions.len()
    );
    ExitCode::SUCCESS
}

fn print_usage() {
    println!("alisp - 极简 Lisp → Aluka ISA 字节码（M4 玩具 DSL 前端）");
    println!();
    println!("用法: alisp <input.lisp> [-o out.aluc] [--format aluc|alua] [--disasm]");
}
