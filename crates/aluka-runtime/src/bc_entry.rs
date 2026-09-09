//! 字节码入口执行（`aluka run *.bc` / `aluvm run` 共用装配）。
//!
//! 装配序：容器魔数嗅探 → Verifier 校验 → Vm 组装（动态求值 provider +
//! 真实 worker 线程钩子 + process.argv 注入 + CJS 上下文）→ 解释执行 →
//! 退出码映射（process.exit / 未捕获异常 / 内部错误）。
//!
//! 独立 bin（aluvm）与本模块保持单一事实来源：bin 仅做参数解析与帮助文本，
//! 全部执行装配在此——统一 `aluka` 单二进制（M7.1）以同库同流程执行字节码。

use std::path::Path;
use std::process::ExitCode;

use aluka_bytecode::BytecodeModule;
use aluka_parser::source_unit::{LanguageRegistry, ModuleKind};
use aluka_vm::{Value, Vm};

/// 执行字节码模块：加载、校验、运行、按退出码映射收尾。
pub fn execute_bc(input: &Path, cli_args: &[String]) -> ExitCode {
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
    // M5.1：真实 worker 线程钩子（装配层独占编译能力）
    install_worker_entry(&mut vm);
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
                // process.exit(code)：正常终止（含事件循环活跃时立即退出）
                aluka_vm::VmError::Exit(code) => {
                    return ExitCode::from(code.clamp(0, 255) as u8);
                }
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

/// 装配真实 worker 线程钩子（`install_worker_entry` 的本 crate 再导出形态）。
fn install_worker_entry(vm: &mut Vm) {
    crate::install_worker_entry(vm);
}

/// 把脚本路径与命令行参数注入 `process.argv`（argv[0]=脚本路径，对齐 Node 语义的脚本段）。
fn inject_process_argv(vm: &mut Vm, input: &Path, cli_args: &[String]) {
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
