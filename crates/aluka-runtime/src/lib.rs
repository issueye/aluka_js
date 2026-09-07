//! 运行时装配：把各 crate 组装成可用的引擎实例。
//!
//! 这是嵌入方与 CLI 接触的核心门面：创建 [`Runtime`]，喂源码/文件路径，拿结果与标准输出。
//! 内部按现代编译执行流水线装配：语言分类（`LanguageRegistry`）➔ 编译与静态优化（`compile_source_unit`）
//! ➔ 规范校验（`verify`）➔ 虚拟机执行（`aluka_vm::Vm`）。

use std::path::Path;

use aluka_builtins::Registry;
use aluka_compiler::{compile, compile_source_unit, optimize_ast};
use aluka_core::Heap;
use aluka_module::Resolver;
use aluka_parser::ast::Program;
use aluka_parser::source_unit::{LanguageRegistry, ModuleKind, SourceUnitError};
use aluka_vm::{Value, Vm, VmError};
use aluka_webapi::Capability;

/// 运行时装配、编译或执行失败的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    /// 文件 IO 读取错误
    Io(String),
    /// 词法与语法解析错误
    Parse(String),
    /// 编译阶段错误
    Compile(String),
    /// 静态字节码校验失败
    Verify(String),
    /// 虚拟机执行期未捕获异常或内部错误
    Vm(VmError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "IO 错误: {msg}"),
            Self::Parse(msg) => write!(f, "解析错误: {msg}"),
            Self::Compile(msg) => write!(f, "编译错误: {msg}"),
            Self::Verify(msg) => write!(f, "字节码校验错误: {msg}"),
            Self::Vm(err) => write!(f, "执行错误: {err}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<VmError> for RuntimeError {
    fn from(err: VmError) -> Self {
        RuntimeError::Vm(err)
    }
}

/// 一个引擎实例：自带堆、全局能力、内置模块与模块解析器。
///
/// 实例之间完全隔离——包括模块解析条件（运行时用 Node 条件，打包器用
/// browser 条件），这一点是刻意的，见 `aluka-module` 的模块文档。
#[derive(Debug)]
pub struct Runtime {
    heap: Heap,
    builtins: Registry,
    resolver: Resolver,
    capabilities: Vec<Capability>,
    stdout_records: Vec<String>,
    uncaught_formatted: Option<String>,
}

impl Runtime {
    /// 按运行时默认配置装配实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            heap: Heap::new(),
            builtins: Registry::with_planned_modules(),
            resolver: Resolver::for_runtime(),
            capabilities: Capability::all().to_vec(),
            stdout_records: Vec::new(),
            uncaught_formatted: None,
        }
    }

    /// 执行一棵已解析的语法树，返回其求值结果（M-1 兼容接口）。
    pub fn evaluate(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let unit = compile(program);
        let mut vm = Vm::new(unit.locals);
        let res = vm.run(&unit.code)?;
        self.stdout_records = vm.stdout_records.clone();
        Ok(res)
    }

    /// 从文件路径加载并执行指定的脚本或模块源码单元。
    ///
    /// 自动根据文件扩展名（`.js`、`.ts`、`.json`、`.adsl` 等）经 `LanguageRegistry`
    /// 分派至对应的解析与编译器，完成静态校验后由虚拟机执行。
    pub fn execute_file(
        &mut self,
        path: &Path,
        args: &[String],
        optimize: bool,
    ) -> Result<Value, RuntimeError> {
        let path_str = path.to_string_lossy();
        let mut unit = LanguageRegistry::global()
            .parse_file(&path_str, ModuleKind::Script)
            .map_err(|e| match e {
                SourceUnitError::ReadError { message, .. } => RuntimeError::Io(message),
                other => RuntimeError::Parse(other.to_string()),
            })?;

        if optimize {
            if let Some(prog) = &mut unit.program {
                optimize_ast(prog);
            }
        }

        let module =
            compile_source_unit(&mut unit).map_err(|e| RuntimeError::Compile(e.to_string()))?;

        module
            .verify()
            .map_err(|e| RuntimeError::Verify(e.to_string()))?;

        let mut vm = Vm::new(0);
        inject_process_argv(&mut vm, path, args);
        vm.setup_cjs(path);

        let run_res = vm.run_module(&module);
        self.stdout_records = vm.stdout_records.clone();
        if let Err(VmError::Thrown(exc)) = &run_res {
            self.uncaught_formatted = Some(format_uncaught_with_vm(&mut vm, *exc, path));
        } else {
            self.uncaught_formatted = None;
        }
        let res = run_res?;
        Ok(res)
    }

    /// 直接从源码字符串执行指定的脚本或模块。
    pub fn execute_source(
        &mut self,
        src: &str,
        path: &str,
        args: &[String],
        optimize: bool,
    ) -> Result<Value, RuntimeError> {
        let mut unit = LanguageRegistry::global()
            .parse_source(src, path, ModuleKind::Script)
            .map_err(|e| RuntimeError::Parse(e.to_string()))?;

        if optimize {
            if let Some(prog) = &mut unit.program {
                optimize_ast(prog);
            }
        }

        let module =
            compile_source_unit(&mut unit).map_err(|e| RuntimeError::Compile(e.to_string()))?;

        module
            .verify()
            .map_err(|e| RuntimeError::Verify(e.to_string()))?;

        let path_buf = Path::new(path);

        let mut vm = Vm::new(0);
        inject_process_argv(&mut vm, path_buf, args);
        vm.setup_cjs(path_buf);

        let run_res = vm.run_module(&module);
        self.stdout_records = vm.stdout_records.clone();
        if let Err(VmError::Thrown(exc)) = &run_res {
            self.uncaught_formatted = Some(format_uncaught_with_vm(&mut vm, *exc, path_buf));
        } else {
            self.uncaught_formatted = None;
        }
        let res = run_res?;
        Ok(res)
    }

    /// 获取最近一次执行产生的格式化未捕获异常文本（若发生异常）。
    #[must_use]
    pub fn uncaught_formatted(&self) -> Option<&str> {
        self.uncaught_formatted.as_deref()
    }

    /// 获取最近一次执行期间由 `console.log` 等写入的标准输出行切片。
    #[must_use]
    pub fn stdout_records(&self) -> &[String] {
        &self.stdout_records
    }

    /// 格式化未捕获异常的友好文本展示。
    #[must_use]
    pub fn format_uncaught(exc: Value, path: &Path) -> String {
        let mut vm = Vm::new(0);
        format_uncaught_with_vm(&mut vm, exc, path)
    }
}

/// 利用拥有完整堆对象的 VM 实例格式化异常。
fn format_uncaught_with_vm(vm: &mut Vm, exc: Value, path: &Path) -> String {
    let msg = if matches!(exc, Value::Object(_)) {
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
            format!("{name}: {message}")
        } else {
            vm.format_value(exc)
        }
    } else {
        vm.format_value(exc)
    };
    format!("{msg}\n    at <module> ({})", path.display())
}

impl Runtime {
    /// 内置模块注册表（迁移期兼作进度看板）。
    #[must_use]
    pub fn builtins(&self) -> &Registry {
        &self.builtins
    }

    /// 模块解析器。
    #[must_use]
    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    /// 已装配的全局能力域。
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// 堆统计快照（分配数、存活数、回收次数）。
    #[must_use]
    pub fn heap_stats(&self) -> aluka_core::gc::GcStats {
        self.heap.stats()
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

/// 把脚本路径与命令行参数注入 `process.argv`（argv[0]=脚本路径，对齐 Node 语义）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use aluka_parser::ast::{Expr, Stmt};

    #[test]
    fn evaluates_an_addition_end_to_end() {
        let program = Program {
            body: vec![Stmt::Expr(Expr::Binary {
                op: "+".to_owned(),
                left: Box::new(Expr::Number(20.0)),
                right: Box::new(Expr::Number(22.0)),
            })],
        };
        let mut runtime = Runtime::new();
        match runtime.evaluate(&program) {
            Ok(Value::Number(n)) => assert_eq!(n, 42.0),
            other => panic!("expected Number(42), got {other:?}"),
        }
    }

    #[test]
    fn assembles_builtins_resolver_and_capabilities() {
        let runtime = Runtime::new();
        assert!(!runtime.builtins().is_empty());
        assert!(runtime.resolver().has_condition("node"));
        assert!(!runtime.resolver().has_condition("browser"));
        assert_eq!(runtime.capabilities().len(), Capability::all().len());
    }

    #[test]
    fn fresh_runtime_has_empty_heap_stats() {
        let runtime = Runtime::new();
        assert_eq!(runtime.heap_stats().allocated, 0);
        assert_eq!(runtime.heap_stats().collections, 0);
    }

    #[test]
    fn test_runtime_execute_source_js_and_ts() {
        let mut runtime = Runtime::new();

        // 1. JavaScript 执行
        let js_src = "const a = 10; const b = 20; console.log('js sum:', a + b);";
        let res = runtime.execute_source(js_src, "test.js", &[], true);
        assert!(res.is_ok());
        assert_eq!(runtime.stdout_records(), &["js sum: 30"]);

        // 2. TypeScript 类型剥离与执行
        let ts_src = r#"
            interface Point { x: number; y: number; }
            function getX(p: Point): number { return p.x; }
            const pt: Point = { x: 100, y: 200 };
            console.log('ts x:', getX(pt));
        "#;
        let res_ts = runtime.execute_source(ts_src, "test.ts", &[], true);
        assert!(res_ts.is_ok());
        assert_eq!(runtime.stdout_records(), &["ts x: 100"]);
    }

    #[test]
    fn test_runtime_execute_source_dsl() {
        let mut runtime = Runtime::new();
        let dsl_src = r#"
            (def a 30)
            (def b 12)
            (console.log "dsl mul:" (* a b))
        "#;
        let res = runtime.execute_source(dsl_src, "calc.adsl", &[], true);
        assert!(res.is_ok());
        assert_eq!(runtime.stdout_records(), &["dsl mul: 360"]);
    }
}
