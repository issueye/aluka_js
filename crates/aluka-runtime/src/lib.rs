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

/// 字节码入口执行装配（`aluka run *.bc` / `aluvm run` 单一事实来源）。
pub mod bc_entry;
/// 测试报告器形态与汇总计数（`aluka test` 子命令与嵌入方共用）。
pub use aluka_vm::builtins::test_reporters::{ReportCounts, ReportStatus, ReporterKind};
pub use bc_entry::execute_bc;

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
    /// 测试运行器报告器（`enable_test_runner` 启用；None = 不自动跑用例）。
    test_reporter: Option<ReporterKind>,
    /// LCOV 覆盖率报告文本（`enable_test_runner(Lcov)` 时由 execute 收尾生成）
    lcov_report: Option<String>,
    /// 行覆盖编译开关（与 lcov 报告器联动；编译器把语句起始登记进函数行表）
    coverage_compile: bool,
    /// 最近一次执行收尾的测试汇总（未启用/无用例/已显式 run() 时为 None）。
    test_summary: Option<ReportCounts>,
    /// 最近一次执行的 `process.exit(code)` 退出码（None = 未调用）。
    exit_code: Option<i32>,
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
            test_reporter: None,
            lcov_report: None,
            coverage_compile: false,
            test_summary: None,
            exit_code: None,
        }
    }

    /// 启用 `node:test` 收尾自动运行（`aluka test` 子命令入口）。
    ///
    /// 启用后，`execute_file`/`execute_source`/`evaluate` 的执行收尾会调用
    /// `aluka_vm::builtins::test::auto_run`：把报告行按序追加到 stdout 记录
    /// （与 `console.log` 同一输出通道），并记录汇总计数供退码判定。
    pub fn enable_test_runner(&mut self, kind: ReporterKind) {
        // 测试运行一律挂载覆盖计数（每指令一次 Option 判定，开销近零）；
        // `run().compose(reporters.lcov)` 因此在任何报告器模式下都有数据
        self.coverage_compile = true;
        self.test_reporter = Some(kind);
    }

    /// 最近一次执行的 LCOV 覆盖率报告（仅 `Lcov` 报告器挂载后产生）。
    #[must_use]
    pub fn lcov_report(&self) -> Option<&str> {
        self.lcov_report.as_deref()
    }

    /// 最近一次执行收尾的测试汇总计数（未启用运行器、注册表为空、脚本已
    /// 显式调用 `test.run()` 时为 `None`）。
    #[must_use]
    pub fn test_summary(&self) -> Option<ReportCounts> {
        self.test_summary
    }

    /// 最近一次执行是否由 `process.exit(code)` 正常终止；是则给出其退出码
    /// （对齐 Node：宿主据此设置进程退出码）。未发生 `process.exit` 时为 `None`。
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// 执行一棵已解析的语法树，返回其求值结果（M-1 兼容接口）。
    pub fn evaluate(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let unit = compile(program);
        let mut vm = Vm::new(unit.locals);
        let res = match vm.run(&unit.code) {
            Ok(v) => v,
            // 同 `execute_file`：`process.exit` 属正常终止（记录退出码）
            Err(VmError::Exit(code)) => {
                self.stdout_records = vm.stdout_records.clone();
                self.exit_code = Some(code);
                return Ok(Value::Undefined);
            }
            Err(e) => return Err(RuntimeError::Vm(e)),
        };
        self.test_summary = auto_test_run(&mut vm, self.test_reporter);
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
        // 按扩展名推断模块种类：.mjs/.mts → ESM（alukac 同款判定）
        let module_kind = match path.extension().and_then(|e| e.to_str()) {
            Some("mjs") | Some("mts") => ModuleKind::Esm,
            _ => ModuleKind::Script,
        };
        let mut unit = LanguageRegistry::global()
            .parse_file(&path_str, module_kind)
            .map_err(|e| match e {
                SourceUnitError::ReadError { message, .. } => RuntimeError::Io(message),
                other => RuntimeError::Parse(other.to_string()),
            })?;

        if optimize {
            if let Some(prog) = &mut unit.program {
                optimize_ast(prog);
            }
        }

        let module = if self.coverage_compile {
            let program = unit.program.take().expect("coverage 编译需要保留 program");
            aluka_compiler::compile_module_with_coverage(&program)
        } else {
            compile_source_unit(&mut unit).map_err(|e| RuntimeError::Compile(e.to_string()))?
        };

        module
            .verify()
            .map_err(|e| RuntimeError::Verify(e.to_string()))?;

        let mut vm = Vm::new(0);
        install_eval_provider(&mut vm);
        install_worker_entry(&mut vm);
        inject_process_argv(&mut vm, path, args);
        vm.setup_cjs(path);

        // LCOV 行覆盖（`aluka test --test-reporter=lcov`）：挂载计数器
        let lcov_module = if self.coverage_compile {
            if std::env::var("ALUKA_LCOV_DEBUG").is_ok() {
                for (i, f) in module.functions.iter().enumerate() {
                    eprintln!("[lcov] func[{i}] {} table={:?}", f.name, f.line_table);
                }
            }
            // 覆盖模式关闭 JIT：热点机器码不经过解释器 tick（否则计数停摆）；
            // 覆盖率运行以诊断为目的，性能让位（登记：Node 覆盖率运行同样减速）
            vm.set_jit_enabled(false);
            vm.coverage = Some(aluka_vm::coverage::Coverage::from_module(&module));
            Some(module.clone())
        } else {
            None
        };

        let run_res = vm.run_module(&module);
        // 测试运行器（`aluka test`）收尾：脚本无未捕获异常时才自动跑用例
        // （文件级失败已足以判定退码，避免半程注册表产出误导性报告）。
        self.test_summary = if run_res.is_ok() {
            auto_test_run(&mut vm, self.test_reporter)
        } else {
            None
        };
        self.stdout_records = vm.stdout_records.clone();
        // LCOV 报告生成（执行结束时的计数快照）
        if let (Some(cov), Some(module)) = (vm.coverage.take(), lcov_module) {
            self.lcov_report = Some(cov.generate_lcov(&module, "", Some(&path_str)));
        }
        if let Err(VmError::Thrown(exc)) = &run_res {
            self.uncaught_formatted = Some(format_uncaught_with_vm(&mut vm, *exc, path));
        } else {
            self.uncaught_formatted = None;
        }
        match run_res {
            Ok(res) => Ok(res),
            // `process.exit(code)` 是**正常终止**（对齐 `bc_entry` 的 `VmError::Exit`
            // 口径与 Node 语义：立即终止、退出码交给宿主），不是未捕获异常。
            Err(VmError::Exit(code)) => {
                self.exit_code = Some(code);
                Ok(Value::Undefined)
            }
            Err(e) => Err(RuntimeError::Vm(e)),
        }
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
        install_eval_provider(&mut vm);
        install_worker_entry(&mut vm);
        inject_process_argv(&mut vm, path_buf, args);
        vm.setup_cjs(path_buf);

        let run_res = vm.run_module(&module);
        // 测试运行器（`aluka test`）收尾：同 `execute_file`（异常时跳过自动运行）。
        self.test_summary = if run_res.is_ok() {
            auto_test_run(&mut vm, self.test_reporter)
        } else {
            None
        };
        self.stdout_records = vm.stdout_records.clone();
        if let Err(VmError::Thrown(exc)) = &run_res {
            self.uncaught_formatted = Some(format_uncaught_with_vm(&mut vm, *exc, path_buf));
        } else {
            self.uncaught_formatted = None;
        }
        match run_res {
            Ok(res) => Ok(res),
            // `process.exit(code)` 是**正常终止**（对齐 `bc_entry` 的 `VmError::Exit`
            // 口径与 Node 语义），不是未捕获异常：记录退出码后按正常返回。
            Err(VmError::Exit(code)) => {
                self.exit_code = Some(code);
                Ok(Value::Undefined)
            }
            Err(e) => Err(RuntimeError::Vm(e)),
        }
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

/// 执行收尾的测试运行器挂钩：启用时调用 VM 侧 `test::auto_run`（把报告行
/// 写入 `vm.stdout_records`，由 CLI 统一打印），返回汇总计数供退码判定；
/// 未启用运行器时返回 `None`（普通 `aluka run` 路径行为完全不变）。
///
/// 分层方向：装配层（aluka-runtime）→ 执行层（aluka-vm），不得反向依赖。
fn auto_test_run(vm: &mut Vm, reporter: Option<ReporterKind>) -> Option<ReportCounts> {
    let kind = reporter?;
    aluka_vm::builtins::test::auto_run(vm, kind)
}

/// 装配动态求值编译器 Hook（eval / new Function）：源码 → 编译 → 字节码。
/// 动态产物在 VM 侧仍强制 Verifier 校验（compile_dynamic 门禁）。
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
        // （闭包回填/类/提升等完整语言特性可用）
        let mut compiler = aluka_compiler::ModuleCompiler {
            preserve_completion_value: true,
            implicit_globals: true,
            ..Default::default()
        };
        let module = compiler.compile(&program);
        Ok(module)
    });
}

// ---------------------------------------------------------------------------
// M5.1 真实跨物理线程 worker：装配层 spawn 钩子
// ---------------------------------------------------------------------------

/// 真实 worker 物理线程 id 计数（对齐 Node `worker.threadId`，自 1 起）。
static WORKER_THREAD_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 装配真实 worker 线程 spawn 钩子（`new Worker` 的物理线程路径）。
///
/// 职责分界：装配层独占编译能力（JS 源码 → 字节码），worker 线程内构建
/// 独立 `Vm`（独立堆 + 线程局部内置表），跨线程只传 JSON 字符串。
pub fn install_worker_entry(vm: &mut Vm) {
    vm.set_worker_entry(std::sync::Arc::new(
        |source: aluka_vm::worker::WorkerSource, worker_data: Option<&str>| {
            let thread_id =
                WORKER_THREAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let (to_worker_tx, to_worker_rx) =
                std::sync::mpsc::channel::<aluka_vm::worker::WorkerInbound>();
            let (from_worker_tx, from_worker_rx) = std::sync::mpsc::channel();
            let terminate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            let data = worker_data.map(str::to_owned);
            let t_terminate = terminate.clone();
            let handle = std::thread::Builder::new()
                .name(format!("aluka-worker-{thread_id}"))
                .spawn(move || {
                    let io = aluka_vm::worker::WorkerThreadIo {
                        thread_id,
                        worker_data_json: data,
                        to_main: from_worker_tx.clone(),
                        from_main: to_worker_rx,
                        terminate: t_terminate,
                    };
                    let code = match source {
                        aluka_vm::worker::WorkerSource::File(path) => run_worker_file(&path, io),
                        aluka_vm::worker::WorkerSource::Eval(src) => run_worker_eval(&src, io),
                    };
                    // 退出码后送（Sender 克隆保属主端存活；Receiver 端在桥上）
                    let _ = from_worker_tx.send(aluka_vm::worker::WorkerEvent::Exit(code));
                });
            handle.map_err(|e| e.to_string())?;

            Ok(aluka_vm::worker::WorkerBridge {
                thread_id,
                to_worker: to_worker_tx,
                from_worker: from_worker_rx,
                terminate,
            })
        },
    ));
}

/// worker 输入解析：`.bc` 直用；`.js/.ts` 等源码形态在同目录存在预编译
/// `.bc`（字节码分发模式）时优先取 `.bc`，否则保留源码路径（现场编译）。
fn resolve_worker_input(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.extension().and_then(|e| e.to_str()) == Some("bc") {
        return path.to_owned();
    }
    let with_bc = p.with_extension("bc");
    if with_bc.is_file() {
        return with_bc.to_string_lossy().to_string();
    }
    let appended = std::path::PathBuf::from(format!("{path}.bc"));
    if appended.is_file() {
        return appended.to_string_lossy().to_string();
    }
    path.to_owned()
}

/// 结构化错误事件载荷（主线程 `'error'` 收到 Error 对象的 name/message 面）。
fn worker_error(name: &str, message: String) -> aluka_vm::worker::WorkerEvent {
    aluka_vm::worker::WorkerEvent::Error {
        name: name.to_owned(),
        message,
    }
}

/// 从抛出的异常值提取 `name`/`message`（缺省回退：name=Error，message=值格式化）。
fn exc_name_message(vm: &mut aluka_vm::interpreter::Vm, exc: aluka_vm::Value) -> (String, String) {
    let name = vm
        .get_property(exc, "name")
        .ok()
        .filter(|v| !matches!(v, aluka_vm::Value::Undefined))
        .map(|v| vm.format_value(v))
        .unwrap_or_else(|| "Error".to_owned());
    let message = vm
        .get_property(exc, "message")
        .ok()
        .filter(|v| !matches!(v, aluka_vm::Value::Undefined))
        .map(|v| vm.format_value(v))
        .unwrap_or_else(|| vm.format_value(exc));
    (name, message)
}

/// worker 线程主体：登记线程 I/O 束 → 加载 worker（字节码容器或源码编译）
/// → 独立 Vm 执行 → 事件循环泵至退出。返回退出码（0 正常；1 异常 / 终止）。
fn run_worker_file(path: &str, io: aluka_vm::worker::WorkerThreadIo) -> u32 {
    // 线程角色登记：此后本线程 `worker_thread_io()` 可用
    aluka_vm::worker::set_worker_thread_io(io);
    let io = aluka_vm::worker::worker_thread_io().expect("上方刚登记");

    let input = resolve_worker_input(path);
    let input_path = std::path::Path::new(&input);
    let is_bc = input.ends_with(".bc");

    // 组装字节码模块：字节码容器直载（字节码分发模式），或源码现场编译
    let module_and_payload: (aluka_bytecode::BytecodeModule, Option<Vec<u8>>) = if is_bc {
        let data = match std::fs::read(input_path) {
            Ok(d) => d,
            Err(err) => {
                let _ = io.to_main.send(worker_error(
                    "Error",
                    format!("worker: 无法读取 {input}: {err}"),
                ));
                return 1;
            }
        };
        match aluka_bytecode::BytecodeModule::load_any_container(&data) {
            Ok((module, range)) => (module, Some(data[range].to_vec())),
            Err(err) => {
                let _ = io.to_main.send(worker_error(
                    "Error",
                    format!("worker: 反序列化 {input} 失败: {err}"),
                ));
                return 1;
            }
        }
    } else {
        // 按扩展名推断模块种类（alukac 同款判定）
        let module_kind = match input_path.extension().and_then(|e| e.to_str()) {
            Some("mjs") | Some("mts") => ModuleKind::Esm,
            _ => ModuleKind::Script,
        };
        let mut unit = match LanguageRegistry::global().parse_file(&input, module_kind) {
            Ok(unit) => unit,
            Err(e) => {
                let _ = io.to_main.send(worker_error(
                    "Error",
                    format!("worker: 无法读取 {input}: {e}"),
                ));
                return 1;
            }
        };
        match compile_source_unit(&mut unit) {
            Ok(m) => (m, None),
            Err(e) => {
                let _ = io.to_main.send(worker_error(
                    "SyntaxError",
                    format!("worker: 编译失败: {e}"),
                ));
                return 1;
            }
        }
    };
    let (module, payload) = module_and_payload;
    if let Err(e) = module.verify() {
        let _ = io.to_main.send(worker_error(
            "Error",
            format!("worker: 字节码校验失败: {e}"),
        ));
        return 1;
    }

    let mut vm = Vm::new(0);
    install_eval_provider(&mut vm);
    // worker 角色表面：parentPort / isMainThread=false / workerData
    aluka_vm::builtins::worker_threads::setup_worker_globals(&mut vm);
    vm.setup_cjs(input_path);

    if let Some(payload) = payload {
        if let Err(err) = vm.load_module(&payload, &module) {
            let _ = io.to_main.send(worker_error(
                "Error",
                format!("worker: functions 标量头不完整: {err}"),
            ));
            return 1;
        }
    }

    match vm.run_module(&module) {
        Ok(_) => aluka_vm::builtins::worker_threads::run_worker_event_loop(&mut vm),
        Err(VmError::Thrown(exc)) => {
            // 主线程 'error' 收到 Error 对象（name/message 保真，对齐 Node）
            let (name, message) = exc_name_message(&mut vm, exc);
            let _ = io.to_main.send(worker_error(&name, message));
            1
        }
        Err(_) => 1,
    }
}

/// eval worker 线程主体（`new Worker(src, { eval: true })`）：源码现场编译
/// → 独立 Vm 执行 → 事件循环泵至退出。Node 22 实测口径：`__filename ===
/// '[worker eval]'`、`__dirname === '.'`，相对 `require` 自 cwd 解析。
fn run_worker_eval(src: &str, io: aluka_vm::worker::WorkerThreadIo) -> u32 {
    // 线程角色登记：此后本线程 `worker_thread_io()` 可用
    aluka_vm::worker::set_worker_thread_io(io);
    let io = aluka_vm::worker::worker_thread_io().expect("上方刚登记");

    let mut unit =
        match LanguageRegistry::global().parse_source(src, "[worker eval]", ModuleKind::Script) {
            Ok(unit) => unit,
            Err(e) => {
                let _ = io.to_main.send(worker_error("SyntaxError", e.to_string()));
                return 1;
            }
        };
    let module = match compile_source_unit(&mut unit) {
        Ok(m) => m,
        Err(e) => {
            let _ = io.to_main.send(worker_error("SyntaxError", e.to_string()));
            return 1;
        }
    };
    if let Err(e) = module.verify() {
        let _ = io.to_main.send(worker_error(
            "Error",
            format!("worker: 字节码校验失败: {e}"),
        ));
        return 1;
    }

    let mut vm = Vm::new(0);
    install_eval_provider(&mut vm);
    aluka_vm::builtins::worker_threads::setup_worker_globals(&mut vm);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    vm.setup_cjs_eval(cwd);

    match vm.run_module(&module) {
        Ok(_) => aluka_vm::builtins::worker_threads::run_worker_event_loop(&mut vm),
        Err(VmError::Thrown(exc)) => {
            let (name, message) = exc_name_message(&mut vm, exc);
            let _ = io.to_main.send(worker_error(&name, message));
            1
        }
        Err(_) => 1,
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
    use aluka_parser::ast::{Expr, SpannedStmt, Stmt};

    #[test]
    fn evaluates_an_addition_end_to_end() {
        let program = Program {
            body: vec![SpannedStmt::new(
                Stmt::Expr(Expr::Binary {
                    op: "+".to_owned(),
                    left: Box::new(Expr::Number(20.0)),
                    right: Box::new(Expr::Number(22.0)),
                }),
                0,
            )],
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
