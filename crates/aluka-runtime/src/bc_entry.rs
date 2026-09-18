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
///
/// `argv[0]` 取 `input`（字节码路径）——`aluvm run app.bc` / `aluka run app.bc`
/// 直执行场景下这正是用户所指的目标。源码经 `aluka run` 构建后执行的场景请用
/// [`execute_bc_with_script`] 传入源脚本路径（否则 `argv[0]` 会暴露内部构建产物）。
pub fn execute_bc(input: &Path, cli_args: &[String]) -> ExitCode {
    execute_bc_with_script(input, None, cli_args)
}

/// 执行字节码模块，并**显式指定 `process.argv[0]`**（源脚本路径）。
///
/// `aluka run <源码>` 时字节码是内部构建产物（`aluka_build/<相对路径>.bc`），
/// 对脚本可见的 `argv[0]` 应是用户实际运行的源脚本，而非 `.bc` 产物。
/// `script` 为 `None` 时退回 `input`。
pub fn execute_bc_with_script(
    input: &Path,
    script: Option<&Path>,
    cli_args: &[String],
) -> ExitCode {
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
    install_source_module_provider(&mut vm);
    // M5.1：真实 worker 线程钩子（装配层独占编译能力）
    install_worker_entry(&mut vm);
    // `argv[0]`：源码构建场景取源脚本，直执行场景取字节码路径
    inject_process_argv(&mut vm, script.unwrap_or(input), cli_args);
    // CJS 模块上下文（require/exports/循环依赖）。源码构建场景以**源脚本**
    // 路径为基准（`__dirname`/`__filename`/相对 require 与 Node 一致——
    // 此前用 .bc 镜像路径，ESM 相对导入的绝对化基准随之错位）；入口
    // 路径按 cwd 绝对化（相对路径的 parent 为空 → `__dirname` 成空串）
    let cjs_base = script.unwrap_or(input);
    let cjs_base_abs = if cjs_base.is_absolute() {
        cjs_base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(cjs_base)
    };
    // 解析基准 = 字节码镜像路径（`aluka_build/` 下才有 .bc 与 node_modules
    // 镜像）；观察基准 = 源码路径（`__dirname`/`import.meta` 用户可见面）
    vm.setup_cjs_dual(input, &cjs_base_abs);
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
/// 装配源模块编译器 Hook（`bc_entry` 姊妹路径，语义见
/// `crate::install_source_module_provider`）。
fn install_source_module_provider(vm: &mut Vm) {
    vm.set_source_module_provider(|file: &std::path::Path| {
        let path_str = file.to_string_lossy();
        let src = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let kind = aluka_compiler::module_kind::module_kind_for_source(file, &src);
        let mut unit = LanguageRegistry::global()
            .parse_source(&src, &path_str, kind)
            .map_err(|e| e.to_string())?;
        crate::compile_source_unit(&mut unit).map_err(|e| e.to_string())
    });
}

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
    if exc.is_object() {
        let name = vm
            .get_property(exc, "name")
            .ok()
            .map(|v| vm.format_value(v))
            .unwrap_or_default();
        // name 缺失（自定义错误类只定义了 toString 时）：回退调用
        // toString —— 官方 assert.throws 判定与 Node 渲染均依赖其
        // 输出含类型名（`Test262Error: ...`）
        if name.is_empty() || name == "undefined" {
            if let Some(t) = vm.call_to_string(exc) {
                if !t.is_empty() && t != "[object Object]" {
                    return t;
                }
            }
        }
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
