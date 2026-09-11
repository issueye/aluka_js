//! `child_process` 内置模块（Phase 6）。
//!
//! 语义逐字对齐 Node.js 22 LTS 标准（Node.js 22 LTS 规范
//! 与 `child_process_sync.go`）：
//! - 异步四件套 `spawn` / `exec` / `execFile` / `fork`：真实 OS 子进程 +
//!   后台读管道线程，事件经 `proc` 事件源泵派发（见 [`proc_common`]，
//!   对齐 Go 的 goroutine + PostTask 模型与事件顺序 spawn→data→end→close→
//!   exit→close）；
//! - 同步三件套 `spawnSync` / `execFileSync` / `execSync`：阻塞执行。
//!   `spawnSync` 出错不抛（结果对象带 `error` 属性），`execFileSync` /
//!   `execSync` 出错抛 JS 异常（Node 语义）；
//! - Windows shell 语义：`exec` 经 `cmd /c <整串>` 执行；`execSync` 按空白
//!   拆分并剥参数两端引号后直接执行（Go 的简化，非 shell 语义）；
//! - 错误字符串逐字对齐 Go：`exit status 5`、`Command failed: <解析后命令行>`、
//!   `exec: "x": executable file not found in %PATH%`（[`go_look_path`] 复刻
//!   Go `exec.LookPath` 的路径解析用于消息）。
//!
//! 子进程实例是 `_builtinNs = "child_process:child"` 的实例事件器
//! （`on/emit/...` 共享实现见 [`proc_common`]），`stdout`/`stderr` 流实例与
//! `stdin` 对象各占独立命名空间。

/// 共享基建（实例事件器 + proc 事件泵）：`worker_threads` / `cluster` 复用。
pub(crate) mod proc_common;

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use proc_common::{
    ProcEvent, StreamKind, ns_attach, push_event, register_ns_emitter_handlers, spawn_pipe_reader,
    with_children,
};
use std::process::{Command, Stdio};

/// `require("child_process")` / `require("node:child_process")` 模块导出。
pub const MODULE: ModuleDef = ModuleDef {
    name: "child_process",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for method in [
        "spawn",
        "exec",
        "execFile",
        "fork",
        "spawnSync",
        "execFileSync",
        "execSync",
    ] {
        let f = vm.alloc_native_fn(&format!("child_process.{method}"));
        set_module_prop(vm, obj, method, Value::Object(f))?;
    }
    register_handler(registry, "child_process", "spawn", cp_spawn);
    register_handler(registry, "child_process", "exec", cp_exec);
    register_handler(registry, "child_process", "execFile", cp_exec_file);
    register_handler(registry, "child_process", "fork", cp_fork);
    register_handler(registry, "child_process", "spawnSync", cp_spawn_sync);
    register_handler(registry, "child_process", "execFileSync", cp_exec_file_sync);
    register_handler(registry, "child_process", "execSync", cp_exec_sync);
    // 子进程 / 流 / stdin 的实例方法命名空间。
    register_ns_emitter_handlers(registry, "child_process:child");
    register_handler(registry, "child_process:child", "kill", child_kill);
    register_ns_emitter_handlers(registry, "child_process:stream");
    register_handler(registry, "child_process:stream", "destroy", stream_destroy);
    register_handler(registry, "child_process:stdin", "write", stdin_write);
    register_handler(registry, "child_process:stdin", "end", stdin_end);
    vm.activate_event_source("proc", proc_common::pump_proc);
    Ok(obj)
}

// ---------------------------------------------------------------------------
// spawn / fork
// ---------------------------------------------------------------------------

/// spawn options 的公共字段（Go spawnChild 的 options 读取）。
pub(crate) struct SpawnOpts {
    /// silent 缺省 → 管道；显式 false → 继承 stdio（fork 默认显式 false）
    pub silent: Option<bool>,
    /// 工作目录（空串 = 继承）
    pub cwd: String,
    /// windowsHide（Go 默认 Windows 下 true）
    pub windows_hide: bool,
    /// 环境变量对（Some = 整体替换环境）
    pub env: Option<Vec<(String, String)>>,
}

fn parse_spawn_opts(vm: &mut Vm, opts_val: Option<Value>) -> SpawnOpts {
    let mut o = SpawnOpts {
        silent: None,
        cwd: String::new(),
        windows_hide: cfg!(windows),
        env: None,
    };
    let Some(opts) = opts_val.and_then(|v| v.as_object()) else {
        return o;
    };
    if let Ok(ValueCase::Boolean(b)) = vm
        .get_property(Value::Object(opts), "silent")
        .map(ValueCase::from)
    {
        o.silent = Some(b);
    }
    if let Ok(v) = vm.get_property(Value::Object(opts), "cwd") {
        o.cwd = heap_string(vm, v).unwrap_or_default();
    }
    if let Ok(ValueCase::Boolean(b)) = vm
        .get_property(Value::Object(opts), "windowsHide")
        .map(ValueCase::from)
    {
        o.windows_hide = b;
    }
    if let Ok(v) = vm.get_property(Value::Object(opts), "env") {
        if v.as_object().is_some() {
            let mut env_list = Vec::new();
            for (k, ev) in vm.own_properties(v) {
                env_list.push((k, vm.format_value(ev)));
            }
            o.env = Some(env_list);
        }
    }
    o
}

/// 构造并启动子进程（Go spawnChild 的 Rust 对应）。
fn spawn_child(
    vm: &mut Vm,
    command: String,
    cmd_args: Vec<String>,
    opts: SpawnOpts,
) -> Result<Value, VmError> {
    // 子进程实例（实例事件器 + kill）。
    let child_obj = vm.alloc_ordinary();
    ns_attach(
        vm,
        child_obj,
        "child_process:child",
        &["on", "once", "off", "emit", "kill", "listenerCount"],
    );

    // silent 缺省（spawn/execFile 路径）→ 管道；显式 silent:false → 继承。
    let inherit_stdio = matches!(opts.silent, Some(false));
    let mut cmd = Command::new(&command);
    cmd.args(&cmd_args);
    if !opts.cwd.is_empty() {
        cmd.current_dir(&opts.cwd);
    }
    if let Some(pairs) = &opts.env {
        cmd.env_clear();
        for (k, v) in pairs {
            cmd.env(k, v);
        }
    }
    apply_windows_hide(&mut cmd, opts.windows_hide);

    let mut stdout_stream: Option<ObjectRef> = None;
    let mut stderr_stream: Option<ObjectRef> = None;
    if inherit_stdio {
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let _ = vm.set_property(Value::Object(child_obj), "stdout", Value::Null);
        let _ = vm.set_property(Value::Object(child_obj), "stderr", Value::Null);
        let _ = vm.set_property(Value::Object(child_obj), "stdin", Value::Null);
    } else {
        for slot in ["stdout", "stderr"] {
            let stream = vm.alloc_ordinary();
            ns_attach(
                vm,
                stream,
                "child_process:stream",
                &["on", "once", "off", "emit", "destroy", "listenerCount"],
            );
            let _ = vm.set_property(Value::Object(stream), "readable", Value::Boolean(true));
            let _ = vm.set_property(
                Value::Object(stream),
                "readableEnded",
                Value::Boolean(false),
            );
            let _ = vm.set_property(Value::Object(stream), "destroyed", Value::Boolean(false));
            let _ = vm.set_property(Value::Object(child_obj), slot, Value::Object(stream));
            if slot == "stdout" {
                stdout_stream = Some(stream);
            } else {
                stderr_stream = Some(stream);
            }
        }
        // stdin：不支持写入（Go 简化一致）；挂命名空间供 CALL_METHOD 分派。
        let stdin = vm.alloc_ordinary();
        ns_attach(vm, stdin, "child_process:stdin", &["write", "end"]);
        let _ = vm.set_property(Value::Object(child_obj), "stdin", Value::Object(stdin));
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    match cmd.spawn() {
        Err(spawn_err) => {
            // Go：PostTask 内先后触发 'error' 与 'exit'(-1)，无 'close'。
            push_event(ProcEvent::SpawnError {
                child: child_obj.0,
                message: go_spawn_error_string(&command, &spawn_err),
            });
        }
        Ok(mut child) => {
            let _ = vm.set_property(
                Value::Object(child_obj),
                "pid",
                Value::Number(child.id() as f64),
            );
            push_event(ProcEvent::Spawn { child: child_obj.0 });
            // 读管道线程：数据块按读序入队，EOF 入队 StreamEof。
            if let Some(pipe) = child.stdout.take() {
                if let Some(sid) = stdout_stream.map(|s| s.0) {
                    spawn_pipe_reader(pipe, child_obj.0, sid, StreamKind::Stdout);
                }
            }
            if let Some(pipe) = child.stderr.take() {
                if let Some(sid) = stderr_stream.map(|s| s.0) {
                    spawn_pipe_reader(pipe, child_obj.0, sid, StreamKind::Stderr);
                }
            }
            let stdout_eof = stdout_stream.is_none();
            let stderr_eof = stderr_stream.is_none();
            with_children(|map| {
                map.insert(
                    child_obj.0,
                    proc_common::ChildState {
                        child,
                        stdout_eof,
                        stderr_eof,
                        exit_enqueued: false,
                    },
                );
            });
        }
    }
    // 事件源幂等激活（泵内部在空闲时自注销）。
    vm.activate_event_source("proc", proc_common::pump_proc);
    Ok(Value::Object(child_obj))
}

/// `child_process.spawn(command[, args][, options])`。
fn cp_spawn(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let command = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let cmd_args = string_list(vm, args.get(1).copied());
    let opts = parse_spawn_opts(vm, args.get(2).copied());
    spawn_child(vm, command, cmd_args, opts)
}

/// `child_process.fork(modulePath[, args][, options])`：spawn 当前可执行文件
/// 跑模块；默认 silent:false → 继承 stdio（Node 语义，Go forkChild 一致）。
fn cp_fork(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let module_path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let fork_args = string_list(vm, args.get(1).copied());
    let mut opts = parse_spawn_opts(vm, args.get(2).copied());
    // Go forkChild 恒写入 silent（缺省 false）→ spawn 侧继承 stdio（Node 语义）。
    opts.silent = Some(matches!(opts.silent, Some(true)));
    fork_spawn(vm, module_path, fork_args, opts)
}

/// fork 的内部实现入口（`cluster.fork` 复用）：spawn 当前可执行文件。
pub(crate) fn fork_spawn(
    vm: &mut Vm,
    module_path: String,
    fork_args: Vec<String>,
    opts: SpawnOpts,
) -> Result<Value, VmError> {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut spawn_args = vec![module_path];
    spawn_args.extend(fork_args);
    spawn_child(vm, exe, spawn_args, opts)
}

// ---------------------------------------------------------------------------
// exec / execFile
// ---------------------------------------------------------------------------

/// exec/execFile 的公共执行：后台线程收集 stdout/stderr 后入队回调事件。
fn run_collect_command(
    vm: &mut Vm,
    mut cmd: Command,
    program: String,
    cb: Value,
) -> Result<(), VmError> {
    apply_windows_hide(&mut cmd, cfg!(windows));
    let finish_task = proc_common::begin_exec_task();
    // 属主线程 id：回调句柄属于发起线程的 Vm 堆，事件须回投给属主
    let owner = std::thread::current().id();
    std::thread::spawn(move || {
        let (err, stdout, stderr) = match cmd.output() {
            Ok(out) => (
                (!out.status.success()).then(|| go_exit_status_string(&out.status)),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ),
            Err(e) => (
                Some(go_spawn_error_string(&program, &e)),
                String::new(),
                String::new(),
            ),
        };
        proc_common::push_event_for(
            owner,
            ProcEvent::ExecDone {
                cb,
                err,
                stdout,
                stderr,
            },
        );
        finish_task();
    });
    vm.activate_event_source("proc", proc_common::pump_proc);
    Ok(())
}

/// `child_process.exec(command[, options][, callback])`：Windows 经
/// `cmd /c <整串>` 执行（Go runtime.GOOS 分支一致），回调参数为字符串。
fn cp_exec(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(command_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let command = vm.format_value(command_val);
    let Some(cb) = find_callback(vm, args, 1) else {
        return Ok(Value::Undefined);
    };
    let mut cmd = Command::new(shell_program());
    cmd.arg(shell_flag()).arg(&command);
    run_collect_command(vm, cmd, shell_program().to_owned(), cb)?;
    Ok(Value::Undefined)
}

/// `child_process.execFile(file[, args][, options][, callback])`。
fn cp_exec_file(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(file_val) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let file = vm.format_value(file_val);
    let (file_args, arg_idx) = match args.get(1) {
        Some(v) if is_array_value(vm, *v) => (string_list(vm, Some(*v)), 2),
        _ => (Vec::new(), 1),
    };
    let Some(cb) = find_callback(vm, args, arg_idx) else {
        return Ok(Value::Undefined);
    };
    let mut cmd = Command::new(&file);
    cmd.args(&file_args);
    run_collect_command(vm, cmd, file, cb)?;
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// 同步三件套
// ---------------------------------------------------------------------------

/// 同步子进程选项（Go spawnSyncOptions）。
struct SyncOpts {
    /// 工作目录
    cwd: String,
    /// 环境变量对（Some = 整体替换）
    env: Option<Vec<(String, String)>>,
    /// 写入 stdin 的字节（Some = 存在 input 选项）
    input: Option<Vec<u8>>,
    /// 超时毫秒（0 = 无）
    timeout: u64,
    /// 输出编码（非空且非 "buffer" → 字符串结果）
    encoding: String,
    /// windowsHide
    windows_hide: bool,
}

fn parse_sync_opts(vm: &mut Vm, opts_val: Option<Value>) -> SyncOpts {
    let mut o = SyncOpts {
        cwd: String::new(),
        env: None,
        input: None,
        timeout: 0,
        encoding: String::new(),
        windows_hide: cfg!(windows),
    };
    let Some(opts) = opts_val.and_then(|v| v.as_object()) else {
        return o;
    };
    if let Ok(v) = vm.get_property(Value::Object(opts), "cwd") {
        o.cwd = heap_string(vm, v).unwrap_or_default();
    }
    if let Ok(v) = vm.get_property(Value::Object(opts), "env") {
        if v.as_object().is_some() {
            let mut env_list = Vec::new();
            for (k, ev) in vm.own_properties(v) {
                env_list.push((k, vm.format_value(ev)));
            }
            o.env = Some(env_list);
        }
    }
    if let Ok(v) = vm.get_property(Value::Object(opts), "input") {
        if !matches!(v, Value::Undefined) {
            o.input = Some(
                crate::builtins::buffer::extract_bytes(vm, v)
                    .unwrap_or_else(|| vm.format_value(v).into_bytes()),
            );
        }
    }
    if let Ok(ValueCase::Number(n)) = vm
        .get_property(Value::Object(opts), "timeout")
        .map(ValueCase::from)
    {
        o.timeout = (n as i64).max(0) as u64;
    }
    if let Ok(v) = vm.get_property(Value::Object(opts), "encoding") {
        o.encoding = heap_string(vm, v).unwrap_or_default();
    }
    if let Ok(ValueCase::Boolean(b)) = vm
        .get_property(Value::Object(opts), "windowsHide")
        .map(ValueCase::from)
    {
        o.windows_hide = b;
    }
    o
}

/// 同步运行命令并构造结果对象（Go runSyncCommand：pid/status/signal/stdout/
/// stderr[/error]；spawnSync 出错不抛）。
fn run_sync_command(
    vm: &mut Vm,
    mut cmd: Command,
    program: &str,
    opts: &SyncOpts,
) -> Result<Value, VmError> {
    if !opts.cwd.is_empty() {
        cmd.current_dir(&opts.cwd);
    }
    if let Some(pairs) = &opts.env {
        cmd.env_clear();
        for (k, v) in pairs {
            cmd.env(k, v);
        }
    }
    apply_windows_hide(&mut cmd, opts.windows_hide);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if opts.input.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // 命令不存在：pid 0 + status null + error{ENOENT}。
            return sync_result_value(
                vm,
                0,
                None,
                None,
                Vec::new(),
                Vec::new(),
                Some(("ENOENT".to_owned(), go_spawn_error_string(program, &e))),
                false,
                opts,
            );
        }
    };
    let pid = child.id();
    if let Some(input) = &opts.input {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = std::io::Write::write_all(&mut stdin, input);
        }
    }
    // 超时：轮询 try_wait，到点主动 Kill（Go 计时器 Kill + 自标 timedOut）。
    let mut timed_out = false;
    if opts.timeout > 0 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(opts.timeout);
        while matches!(child.try_wait(), Ok(None)) {
            if std::time::Instant::now() >= deadline {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let output = match child.wait_with_output() {
        Ok(out) => out,
        Err(_) => {
            return sync_result_value(
                vm,
                pid,
                None,
                None,
                Vec::new(),
                Vec::new(),
                Some(("ENOENT".to_owned(), "exit status -1".to_owned())),
                false,
                opts,
            );
        }
    };
    let (status, signal, error) = if timed_out {
        // 超时：status null + signal SIGTERM + error{ETIMEDOUT}（Node Windows 语义）。
        (
            None,
            Some("SIGTERM".to_owned()),
            Some(("ETIMEDOUT".to_owned(), "exit status 1".to_owned())),
        )
    } else {
        match output.status.code() {
            Some(c) => (Some(c), None, None),
            None => (
                Some(-1),
                Some("SIGKILL".to_owned()),
                Some(("ENOENT".to_owned(), "exit status -1".to_owned())),
            ),
        }
    };
    sync_result_value(
        vm,
        pid,
        status,
        signal,
        output.stdout,
        output.stderr,
        error,
        timed_out,
        opts,
    )
}

/// 构造 spawnSync 结果 JS 对象（encoding 非 buffer → 字符串输出）。
#[allow(clippy::too_many_arguments)]
fn sync_result_value(
    vm: &mut Vm,
    pid: u32,
    status: Option<i32>,
    signal: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<(String, String)>,
    timed_out: bool,
    opts: &SyncOpts,
) -> Result<Value, VmError> {
    let obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "pid", Value::Number(pid as f64));
    let status_val = status.map_or(Value::Null, |c| Value::Number(c as f64));
    let _ = vm.set_property(Value::Object(obj), "status", status_val);
    let signal_val = signal
        .map(|s| Value::Object(vm.alloc_string(s)))
        .unwrap_or(Value::Null);
    let _ = vm.set_property(Value::Object(obj), "signal", signal_val);
    let as_string = !opts.encoding.is_empty() && opts.encoding != "buffer";
    let stdout_val = bytes_output(vm, &stdout, as_string);
    let _ = vm.set_property(Value::Object(obj), "stdout", stdout_val);
    let stderr_val = bytes_output(vm, &stderr, as_string);
    let _ = vm.set_property(Value::Object(obj), "stderr", stderr_val);
    if let Some((code, message)) = error {
        let err_obj = vm.alloc_ordinary();
        let code_val = vm.alloc_string(code);
        let _ = vm.set_property(Value::Object(err_obj), "code", Value::Object(code_val));
        let message_val = vm.alloc_string(message);
        let _ = vm.set_property(
            Value::Object(err_obj),
            "message",
            Value::Object(message_val),
        );
        let _ = vm.set_property(Value::Object(obj), "error", Value::Object(err_obj));
        if timed_out {
            let _ = vm.set_property(Value::Object(obj), "status", Value::Null);
            let sig_val = vm.alloc_string("SIGTERM".to_owned());
            let _ = vm.set_property(Value::Object(obj), "signal", Value::Object(sig_val));
        }
    }
    Ok(Value::Object(obj))
}

/// 字节输出：`as_string` 时转字符串，否则 Buffer（Go gbuffer.NewBufferInstance）。
fn bytes_output(vm: &mut Vm, bytes: &[u8], as_string: bool) -> Value {
    if as_string {
        Value::Object(vm.alloc_string(String::from_utf8_lossy(bytes).to_string()))
    } else {
        Value::Object(crate::builtins::buffer::create_buffer_instance(
            vm,
            bytes.to_vec(),
        ))
    }
}

/// `child_process.spawnSync(command[, args][, options])`：出错不抛。
fn cp_spawn_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let command = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let cmd_args = string_list(vm, args.get(1).copied());
    let opts = parse_sync_opts(vm, args.get(2).copied());
    let mut cmd = Command::new(&command);
    cmd.args(&cmd_args);
    run_sync_command(vm, cmd, &command, &opts)
}

/// 同步抛错对象（Go execError：code/message/status/killed 四属性）。
fn throw_exec_error(
    vm: &mut Vm,
    code: &str,
    message: String,
    status: i32,
) -> Result<Value, VmError> {
    let err = vm.alloc_error_instance(&message);
    let code_val = vm.alloc_string(code.to_owned());
    let _ = vm.set_property(Value::Object(err), "code", Value::Object(code_val));
    let _ = vm.set_property(Value::Object(err), "status", Value::Number(status as f64));
    let _ = vm.set_property(
        Value::Object(err),
        "killed",
        Value::Boolean(code == "ETIMEDOUT"),
    );
    Err(VmError::Thrown(Value::Object(err)))
}

/// execFileSync/execSync 的抛错语义（Go resultError）：结果带 error 属性 → 抛
/// `spawnSync <cmdline> <code>`；非零退出 → 抛 `Command failed: <cmdline>`；
/// 成功 → 按 encoding 返回 stdout（utf8/utf-8 → 字符串，否则 Buffer）。
fn sync_result_or_throw(
    vm: &mut Vm,
    result: Value,
    cmdline_for_error: String,
    opts: &SyncOpts,
) -> Result<Value, VmError> {
    if let Some(err_obj) = vm.get_property(result, "error")?.as_object() {
        let mut code = "ENOENT".to_owned();
        if let Ok(ValueCase::Object(c)) = vm
            .get_property(Value::Object(err_obj), "code")
            .map(ValueCase::from)
        {
            code = heap_string(vm, Value::Object(c)).unwrap_or(code);
        }
        let message = format!("spawnSync {cmdline_for_error} {code}");
        return throw_exec_error(vm, &code, message, -1);
    }
    let status = match vm.get_property(result, "status")?.case() {
        ValueCase::Number(n) => n as i32,
        _ => -1,
    };
    if status != 0 {
        return throw_exec_error(
            vm,
            "",
            format!("Command failed: {cmdline_for_error}"),
            status,
        );
    }
    let out = vm.get_property(result, "stdout")?;
    if opts.encoding == "utf8" || opts.encoding == "utf-8" {
        if let Some(bytes) = crate::builtins::buffer::extract_bytes(vm, out) {
            return Ok(Value::Object(
                vm.alloc_string(String::from_utf8_lossy(&bytes).to_string()),
            ));
        }
    }
    Ok(out)
}

/// `child_process.execFileSync(file[, args][, options])`。
fn cp_exec_file_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(file_val) = args.first().copied() else {
        return throw_exec_error(vm, "", "execFileSync: command required".to_owned(), -1);
    };
    let file = vm.format_value(file_val);
    let cmd_args = match args.get(1) {
        Some(v) if is_array_value(vm, *v) => string_list(vm, Some(*v)),
        _ => Vec::new(),
    };
    let opts = parse_sync_opts(vm, args.get(2).copied());
    let mut cmd = Command::new(&file);
    cmd.args(&cmd_args);
    let result = run_sync_command(vm, cmd, &file, &opts)?;
    sync_result_or_throw(vm, result, go_cmd_string(&file, &cmd_args), &opts)
}

/// `child_process.execSync(command[, options])`：Windows 上按空白拆分并剥
/// 参数两端引号后直接执行（Go 简化）；POSIX 用 `/bin/sh -c`。返回值默认
/// Buffer，`encoding: 'utf8'` 时为字符串；失败抛 JS 异常。
fn cp_exec_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(command_val) = args.first().copied() else {
        return throw_exec_error(vm, "", "execSync: command required".to_owned(), -1);
    };
    let command = vm.format_value(command_val);
    let opts = parse_sync_opts(vm, args.get(1).copied());
    if cfg!(windows) {
        let parts: Vec<String> = command
            .split_whitespace()
            .map(|p| p.trim_matches('"').to_owned())
            .collect();
        let Some(program) = parts.first().cloned() else {
            return throw_exec_error(vm, "", "execSync: empty command".to_owned(), -1);
        };
        let rest: Vec<String> = parts[1..].to_vec();
        let mut cmd = Command::new(&program);
        cmd.args(&rest);
        let result = run_sync_command(vm, cmd, &program, &opts)?;
        sync_result_or_throw(vm, result, go_cmd_string(&program, &rest), &opts)
    } else {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&command);
        let result = run_sync_command(vm, cmd, "/bin/sh", &opts)?;
        sync_result_or_throw(vm, result, format!("/bin/sh -c {command}"), &opts)
    }
}

// ---------------------------------------------------------------------------
// 实例专属方法
// ---------------------------------------------------------------------------

/// `child.kill()`：终止进程，返回是否成功。
fn child_kill(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    let ValueCase::Object(r) = receiver.case() else {
        return Ok(Value::Boolean(false));
    };
    let ok = with_children(|map| {
        if let Some(st) = map.get_mut(&r.0) {
            st.child.kill().is_ok()
        } else {
            false
        }
    });
    Ok(Value::Boolean(ok))
}

/// `stream.destroy()`：置 destroyed 并返回流本身（Go destroyOnce 语义简化）。
fn stream_destroy(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    if receiver.as_object().is_some() {
        let _ = vm.set_property(receiver, "destroyed", Value::Boolean(true));
    }
    Ok(receiver)
}

/// `stdin.write()`：不支持写入，恒返回 false（Go 简化一致）。
fn stdin_write(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// `stdin.end()`。
fn stdin_end(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------------------
// Go 行为复刻辅助
// ---------------------------------------------------------------------------

/// Windows 下的 shell 程序与 flag（Go runtime.GOOS 分支）。
fn shell_program() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

/// shell 的命令 flag：`/c`（Windows）或 `-c`（POSIX）。
fn shell_flag() -> &'static str {
    if cfg!(windows) { "/c" } else { "-c" }
}

/// Windows 下置 CREATE_NO_WINDOW（对齐 Go proc_attr_windows 的 windowsHide）。
#[cfg(windows)]
fn apply_windows_hide(cmd: &mut Command, hide: bool) {
    use std::os::windows::process::CommandExt;
    if hide {
        cmd.creation_flags(0x0800_0000);
    }
}

/// 非 Windows 无窗口概念，空实现。
#[cfg(not(windows))]
fn apply_windows_hide(_cmd: &mut Command, _hide: bool) {}

/// Go exec.Cmd.String()：解析后路径 + 参数以空格连接。
pub(crate) fn go_cmd_string(program: &str, args: &[String]) -> String {
    let mut s = go_look_path(program).unwrap_or_else(|| program.to_owned());
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// Go ExitError.Error()：`exit status N` / `signal: killed`。
fn go_exit_status_string(status: &std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "signal: killed".to_owned(),
        |c| format!("exit status {c}"),
    )
}

/// Go spawn 失败错误串：找不到命令时为
/// `exec: "<name>": executable file not found in %PATH%`。
fn go_spawn_error_string(command: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!("exec: \"{command}\": executable file not found in %PATH%")
    } else {
        err.to_string()
    }
}

/// 复刻 Go `exec.LookPath`（Windows）：PATH 目录 × PATHEXT 扩展（小写化）
/// 逐个探测，命中即返回构造路径（目录取 PATH 原样，对齐 Go 1.25 实测）。
#[must_use]
pub(crate) fn go_look_path(file: &str) -> Option<String> {
    if file.contains('\\') || file.contains('/') {
        return std::path::Path::new(file)
            .is_file()
            .then(|| file.to_owned());
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    let exts: Vec<String> = match std::env::var("PATHEXT") {
        Ok(v) if !v.is_empty() => v
            .split(';')
            .filter(|e| e.starts_with('.'))
            .map(|e| e.to_ascii_lowercase())
            .collect(),
        _ => vec![
            ".com".to_owned(),
            ".exe".to_owned(),
            ".bat".to_owned(),
            ".cmd".to_owned(),
        ],
    };
    for dir in path_env.split(';') {
        let dir = if dir.is_empty() { "." } else { dir };
        for ext in &exts {
            let candidate = format!("{dir}\\{file}{ext}");
            if std::path::Path::new(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 取数组值实参的字符串元素列表（Go args[1].(*ArrayValue) 分支）。
fn string_list(vm: &mut Vm, val: Option<Value>) -> Vec<String> {
    let Some(r) = val.and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let Some(HeapObject::Array { elements, .. }) = vm.heap.get(r.0 as usize) else {
        return Vec::new();
    };
    elements.iter().map(|e| vm.format_value(*e)).collect()
}

/// 判断值是否为数组堆对象。
fn is_array_value(vm: &Vm, val: Value) -> bool {
    let ValueCase::Object(r) = val.case() else {
        return false;
    };
    matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Array { .. }))
}

/// 从 args[from..] 中找第一个可调用值回调（Go cb 扫描规则）。
fn find_callback(vm: &Vm, args: &[Value], from: usize) -> Option<Value> {
    args.iter()
        .skip(from)
        .find(|v| is_callable_value(vm, **v))
        .copied()
}

/// 判断值是否为可调用堆对象（闭包 / 原生函数 / 原生构造器）。
fn is_callable_value(vm: &Vm, val: Value) -> bool {
    let ValueCase::Object(r) = val.case() else {
        return false;
    };
    matches!(
        vm.heap.get(r.0 as usize),
        Some(
            HeapObject::Closure { .. }
                | HeapObject::NativeFn { .. }
                | HeapObject::NativeCtor { .. }
        )
    )
}

/// 取字符串堆对象的文本（非字符串返回 None）。
fn heap_string(vm: &Vm, v: Value) -> Option<String> {
    let ValueCase::Object(r) = v.case() else {
        return None;
    };
    match vm.heap.get(r.0 as usize) {
        Some(HeapObject::String(s)) => Some(s.clone()),
        _ => None,
    }
}
