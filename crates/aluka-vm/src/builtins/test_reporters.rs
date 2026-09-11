//! `test/reporters` 内置模块（Phase 8；M5.4 升级为真 `stream.Transform` 形态）。
//!
//! 移植 Node.js 22 LTS 标准（`nodetest/test_reporters.go` 模块面 + `cmd/aluka`
//! `printTestLine`/汇总格式）：
//! - 模块面（Node 22.23.1 实测口径）：`dot`/`junit`/`spec`/`tap` 为工厂函数，
//!   调用/构造均返回报告器实例；`lcov` 为预构造实例（object，不可 new）；
//!   导出函数名对齐 Node 内部怪癖（`spec.name === 'value'`、
//!   `tap.name === 'tapReporter'`、`junit.name === 'junitReporter'`、
//!   `dot.name === 'dot'`）；
//! - 实例面：报告器实例是真 `stream.Transform` 实例（真 prototype 链——
//!   `instanceof Transform` 成立；`spec` 实例 `constructor.name === 'SpecReporter'`、
//!   `writableObjectMode === true`）；方法 `write`/`end`/`on`/`pipe`；
//!   `end` 触发 `finish`+`close`；
//! - 格式化面：`write` 收到**事件形态分块**（`{type: 'test:pass'|…, data}`）
//!   时增量格式化为本仓报告契约行并转发 `data` 事件 + 返回该文本（供
//!   `TestsStream.compose` 管道消费）；非事件形态分块保持原 `data` 透传。
//!   报告格式源自本仓 CLI `printTestLine`/汇总格式，**不声称与
//!   `node --test` 逐字一致**（Node 侧 duration_ms/stack 本身非确定值）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// `require("test/reporters")` / `require("node:test/reporters")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "test/reporters",
    build,
};

/// 单用例的展示状态（对齐 Go CLI 的 `ok`/`not ok` 二值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportStatus {
    /// 通过（含 skip/todo 的 ok 语义）。
    Ok,
    /// 失败。
    NotOk,
}

/// 报告器形态（`aluka test --test-reporter=<spec|tap|dot>`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReporterKind {
    /// spec：`ok    <name>` 逐用例行 + `ℹ tests N` 汇总（默认）。
    Spec,
    /// tap：`ok 1 - <name>` 逐用例行 + `# tests N` 汇总。
    Tap,
    /// dot：`.``X` 逐用例标记 + 失败清单块。
    Dot,
    /// lcov：LCOV tracefile 覆盖率报告（行覆盖 + 函数覆盖；BRDA 不支持——
    /// 引擎无分支级插桩，登记偏离）。
    Lcov,
}

/// 单用例的报告输入（与 VM 解耦：CLI 只依赖展示面数据，不依赖解释器类型）。
#[derive(Clone, Debug)]
pub struct ReportCase {
    /// 展示名（完整名，套件内为 "suite > case"）。
    pub name: String,
    /// 展示状态（`ok`/`not ok` 二值）。
    pub status: ReportStatus,
    /// 备注（`# SKIP`/`# TODO`/空串）。
    pub note: String,
    /// 失败消息（通过用例为 None）。
    pub error: Option<String>,
}

/// 按报告器形态生成逐用例行（tap 的序号从 1 起）。
#[must_use]
pub fn format_report_lines(cases: &[ReportCase], kind: ReporterKind) -> Vec<String> {
    match kind {
        ReporterKind::Spec => cases
            .iter()
            .map(|c| format_spec_line(c.status, &c.name, &c.note, c.error.as_deref()))
            .collect(),
        ReporterKind::Tap => cases
            .iter()
            .enumerate()
            .map(|(i, c)| format_tap_line(i + 1, c.status, &c.name, &c.note, c.error.as_deref()))
            .collect(),
        // dot：每用例一个无换行标记（`.` 通过 / `X` 失败），由调用方拼接输出。
        ReporterKind::Dot => cases
            .iter()
            .map(|c| match c.status {
                ReportStatus::Ok => ".".to_owned(),
                ReportStatus::NotOk => "X".to_owned(),
            })
            .collect(),
        // lcov：覆盖率报告走 `Coverage::generate_lcov` 专用通道，无逐用例行
        ReporterKind::Lcov => Vec::new(),
    }
}

/// 按报告器形态生成汇总块（spec → `format_spec_summary`；tap →
/// `format_tap_summary`；dot → `format_dot_failed`）。
///
/// `failed` 仅 dot 形态使用（失败清单）；空串表示无需输出。
#[must_use]
pub fn format_summary(counts: &ReportCounts, failed: &[String], kind: ReporterKind) -> String {
    match kind {
        ReporterKind::Spec => format_spec_summary(counts),
        ReporterKind::Tap => format_tap_summary(counts),
        ReporterKind::Dot => format_dot_failed(failed),
        // lcov：同上，专用通道
        ReporterKind::Lcov => String::new(),
    }
}

/// spec 报告器单用例行（对齐 Go `printTestLine` 非 tap 分支）：
/// `ok    <name>( <note>)` / `not ok <name>( <note>)[\n       <error>]`。
pub fn format_spec_line(
    status: ReportStatus,
    name: &str,
    note: &str,
    error: Option<&str>,
) -> String {
    let note = if note.is_empty() {
        String::new()
    } else {
        format!(" ({})", note.trim_start_matches("# "))
    };
    if matches!(status, ReportStatus::Ok) {
        format!("ok    {name}{note}")
    } else {
        let mut line = format!("not ok {name}{note}");
        if let Some(err) = error {
            line.push_str(&format!("\n       {err}"));
        }
        line
    }
}

/// TAP 报告器单用例行（对齐 Go `printTestLine` tap 分支：全局序号 +
/// `# SKIP`/`# TODO` note + `--- message` 块）。
pub fn format_tap_line(
    index: usize,
    status: ReportStatus,
    name: &str,
    note: &str,
    error: Option<&str>,
) -> String {
    let status_word = match status {
        ReportStatus::Ok => "ok",
        ReportStatus::NotOk => "not ok",
    };
    let mut line = if note.is_empty() {
        format!("{status_word} {index} - {name}")
    } else {
        format!("{status_word} {index} - {name} {}", format_note_tap(note))
    };
    if let Some(err) = error {
        line.push_str(&format!("\n  ---\n  message: {err}\n  ..."));
    }
    line
}

/// TAP note 渲染（`# SKIP` → `# SKIP`；对齐 Go：note 直接跟在名字后）。
fn format_note_tap(note: &str) -> String {
    note.trim_start_matches("# ").to_owned()
}

/// 报告汇总计数（对齐 Go CLI 汇总块）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ReportCounts {
    /// 通过数。
    pub pass: u32,
    /// 失败数。
    pub fail: u32,
    /// 取消数。
    pub cancelled: u32,
    /// 跳过数。
    pub skipped: u32,
    /// 待办数。
    pub todo: u32,
}

impl ReportCounts {
    /// 总用例数（pass + fail + cancelled + skipped + todo——对齐 Go）。
    #[must_use]
    pub fn total(&self) -> u32 {
        self.pass + self.fail + self.cancelled + self.skipped + self.todo
    }
}

/// spec 默认报告器汇总块（对齐 Go：`ℹ tests N` 六行；前导空行由调用方拼接）。
#[must_use]
pub fn format_spec_summary(c: &ReportCounts) -> String {
    format!(
        "\nℹ tests {}\nℹ pass  {}\nℹ fail  {}\nℹ cancelled  {}\nℹ skipped  {}\nℹ todo  {}",
        c.total(),
        c.pass,
        c.fail,
        c.cancelled,
        c.skipped,
        c.todo
    )
}

/// tap 报告器汇总块（对齐 Go：`# tests N` 六行；前导空行由调用方拼接）。
#[must_use]
pub fn format_tap_summary(c: &ReportCounts) -> String {
    format!(
        "\n# tests {}\n# pass  {}\n# fail  {}\n# cancelled  {}\n# skipped  {}\n# todo  {}",
        c.total(),
        c.pass,
        c.fail,
        c.cancelled,
        c.skipped,
        c.todo
    )
}

/// dot 报告器失败清单块（对齐 Go：`Failed tests:` + `✖ <full name>` 行）。
#[must_use]
pub fn format_dot_failed(failed: &[String]) -> String {
    if failed.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nFailed tests:\n");
    for name in failed {
        out.push_str(&format!("✖ {name}\n"));
    }
    out
}

/// 报告器实例的增量格式化状态（`write` 事件分块驱动）。
struct ReporterState {
    /// 报告器形态（决定格式化分支）
    kind: &'static str,
    /// TAP 全局序号（`ok N - name`）
    index: usize,
    /// 汇总计数
    counts: ReportCounts,
    /// 失败用例名清单（dot 失败块用）
    failed: Vec<String>,
    /// TAP 头是否已发出（`TAP version 13`，首个输出前一次性）
    header_done: bool,
    /// 是否已 end（此后写入丢弃）
    ended: bool,
}

impl ReporterState {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            index: 0,
            counts: ReportCounts::default(),
            failed: Vec::new(),
            header_done: false,
            ended: false,
        }
    }
}

thread_local! {
    /// 报告器实例状态表（实例对象句柄 id → 状态）。
    static REPORTER_STATES: RefCell<HashMap<u32, ReporterState>> = RefCell::new(HashMap::new());
    /// 报告器类原型（`constructor.name` 面 + Transform 链），按形态建一份。
    static REPORTER_PROTOS: RefCell<HashMap<&'static str, ObjectRef>> = RefCell::new(HashMap::new());
}

/// 报告器形态 → 实例 `constructor.name`（Node 22.23.1 实测）。
const CLASS_NAMES: [(&str, &str); 5] = [
    ("spec", "SpecReporter"),
    ("tap", "TapReporter"),
    ("dot", "DotReporter"),
    ("junit", "JunitReporter"),
    ("lcov", "LcovReporter"),
];

/// 构建 test/reporters 模块导出对象（Node 22.23.1 实测口径）。
///
/// 导出名即 Node 内部工厂名（`spec.name === 'value'` 等怪癖原样保留）；
/// 工厂可调用可构造（Node 对 tap/dot/junit 的 `new` 抛 TypeError——不复刻，
/// 登记见模块文档头）。`lcov` 为预构造实例。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let m = vm.alloc_ordinary();

    // 流方法分派键（实例方法按名称分派，键在 build 期一次性登记）。
    register_handler(registry, "test/reporters:stream", "write", reporter_write);
    register_handler(registry, "test/reporters:stream", "end", reporter_end);
    register_handler(registry, "test/reporters:stream", "pipe", reporter_pipe);

    // 四个工厂报告器：导出名（= NativeFn 名 = 分派键）→ 形态。导出名即
    // 完整分派键（`new X()` 与 `X()` 都按 NativeFn 名 lookup 命中）。
    for (export_name, kind) in [
        ("value", "spec"),
        ("tapReporter", "tap"),
        ("dot", "dot"),
        ("junitReporter", "junit"),
    ] {
        let ctor = vm.alloc_native_fn(export_name);
        set_module_prop(vm, m, kind, Value::Object(ctor))?;
        let handler = match kind {
            "spec" => reporter_ctor_spec,
            "tap" => reporter_ctor_tap,
            "dot" => reporter_ctor_dot,
            _ => reporter_ctor_junit,
        };
        registry.dispatch.insert(export_name.to_owned(), handler);
    }

    // lcov：预构造实例（Node 22：object，不可 new）。
    let lcov = new_reporter_instance(vm, "lcov");
    set_module_prop(vm, m, "lcov", Value::Object(lcov))?;

    Ok(m)
}

/// 报告器构造调用（工厂直调 / `new` 同形）：返回对应形态的 Transform 实例。
fn reporter_ctor_generic(vm: &mut Vm, kind: &'static str) -> Result<Value, VmError> {
    Ok(Value::Object(new_reporter_instance(vm, kind)))
}

fn reporter_ctor_spec(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    reporter_ctor_generic(vm, "spec")
}

fn reporter_ctor_tap(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    reporter_ctor_generic(vm, "tap")
}

fn reporter_ctor_dot(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    reporter_ctor_generic(vm, "dot")
}

fn reporter_ctor_junit(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    reporter_ctor_generic(vm, "junit")
}

/// 创建报告器实例：真 `stream.Transform` 实例（prototype 链挂类原型，
/// 类原型再挂 Transform.prototype——`instanceof Transform` 判定依据），
/// 自有 `constructor`（类函数，`constructor.name` 面）与
/// `writableObjectMode === true`（Node 22 实测）。
fn new_reporter_instance(vm: &mut Vm, kind: &'static str) -> ObjectRef {
    let transform_proto = crate::builtins::stream::transform_prototype();
    let class_name = CLASS_NAMES
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, n)| *n)
        .unwrap_or("Reporter");
    // 类函数 + 类原型（缺 Transform 原型时退化为普通实例——防御性）
    let class_fn = vm.alloc_native_fn(class_name);
    let instance = if let Some(tp) = transform_proto {
        let class_proto = vm.alloc_ordinary_with_exact_proto(Some(tp));
        let _ = vm.set_property(
            Value::Object(class_proto),
            "constructor",
            Value::Object(class_fn),
        );
        vm.set_native_fn_property(class_fn, "prototype", Value::Object(class_proto));
        vm.alloc_ordinary_with_exact_proto(Some(class_proto))
    } else {
        vm.alloc_ordinary()
    };
    let _ = vm.set_property(
        Value::Object(instance),
        "writableObjectMode",
        Value::Boolean(true),
    );
    let kind_str = vm.alloc_string(kind.to_owned());
    let _ = vm.set_property(
        Value::Object(instance),
        "_reporterKind",
        Value::Object(kind_str),
    );

    // 流方法（与既有 Go 形态同名同分派键）；事件器方法显式挂属性
    // （分派到 events:instance 同名处理器，接收者 = 本实例）。
    for m in [
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "removeAllListeners",
        "listenerCount",
        "setMaxListeners",
        "getMaxListeners",
        "prependListener",
        "prependOnceListener",
        "eventNames",
        "listeners",
        "rawListeners",
    ] {
        let key = format!("events:instance.{m}");
        if let Some(h) = vm.builtin_registry.lookup(&key) {
            register_handler(&mut vm.builtin_registry, "test/reporters:stream", m, h);
        }
        let fn_ref = vm.alloc_native_fn(&format!("test/reporters:stream.{m}"));
        let _ = vm.set_property(Value::Object(instance), m, Value::Object(fn_ref));
    }
    let write_fn = vm.alloc_native_fn("test/reporters:stream.write");
    let _ = vm.set_property(Value::Object(instance), "write", Value::Object(write_fn));
    let end_fn = vm.alloc_native_fn("test/reporters:stream.end");
    let _ = vm.set_property(Value::Object(instance), "end", Value::Object(end_fn));
    let pipe_fn = vm.alloc_native_fn("test/reporters:stream.pipe");
    let _ = vm.set_property(Value::Object(instance), "pipe", Value::Object(pipe_fn));

    REPORTER_STATES.with(|g| {
        g.borrow_mut().insert(instance.0, ReporterState::new(kind));
    });
    instance
}

/// 从事件分块（`{type, data}`）提取事件名 / 用例名 / 错误文本。
fn extract_event_parts(vm: &mut Vm, chunk: Value) -> Option<(String, String, Option<String>)> {
    let Value::Object(_) = chunk else {
        return None;
    };
    let type_val = vm.get_property(chunk, "type").ok()?;
    if !vm.is_string_value(type_val) {
        return None;
    }
    let ev_type = vm.format_value(type_val);
    let data = vm.get_property(chunk, "data").unwrap_or(Value::Undefined);
    let mut name = String::new();
    let mut error = None;
    if let Ok(details) = vm.get_property(data, "details") {
        if let Ok(err) = vm.get_property(details, "error") {
            if vm.is_string_value(err) {
                error = Some(vm.format_value(err));
            }
        }
    }
    if let Ok(name_val) = vm.get_property(data, "name") {
        if vm.is_string_value(name_val) {
            name = vm.format_value(name_val);
        }
    }
    Some((ev_type, name, error))
}

/// `write(chunk)`：
/// - **事件形态分块**（`{type: 'test:*'|'end', data}`）：增量格式化为本仓
///   报告契约文本，转发 `data` 事件并**返回该文本**（`TestsStream.compose`
///   的管道消费形态；TAP 首个输出前补 `TAP version 13` 头，`end` 出汇总块）；
/// - 其余分块：保持既有 `data` 透传，返回 `true`（背压已接受）。
fn reporter_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (w, w_id) = match crate::builtins::current_receiver() {
        Value::Object(r) => (Value::Object(r), r.0),
        _ => return Ok(Value::Boolean(true)),
    };
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    let data_ev = ev_name("data", vm);
    let emit_fn = vm.get_property(w, "emit")?;
    let Some((ev_type, name, error)) = extract_event_parts(vm, chunk) else {
        vm.invoke_callable(emit_fn, w, &[data_ev, chunk])?;
        return Ok(Value::Boolean(true));
    };
    // 提取在先（避免与状态表借用冲突）：跳过/待办注记
    let (status, note, counts_field) = match ev_type.as_str() {
        "test:pass" => (ReportStatus::Ok, "", 0u8),
        "test:skip" => (ReportStatus::Ok, "# SKIP", 1),
        "test:todo" => (ReportStatus::Ok, "# TODO", 2),
        "test:fail" => (ReportStatus::NotOk, "", 3),
        _ => (ReportStatus::Ok, "", 9), // start/plan/end 不动计数
    };
    let text = {
        let mut out = String::new();
        REPORTER_STATES.with(|g| {
            let mut states = g.borrow_mut();
            let Some(st) = states.get_mut(&w_id) else {
                return;
            };
            if st.ended {
                return;
            }
            match ev_type.as_str() {
                "test:start" if st.kind == "tap" => {
                    out = format!("# Subtest: {name}\n");
                }
                "test:pass" | "test:skip" | "test:todo" | "test:fail" => {
                    st.index += 1;
                    match counts_field {
                        0 => st.counts.pass += 1,
                        1 => st.counts.skipped += 1,
                        2 => st.counts.todo += 1,
                        3 => {
                            st.counts.fail += 1;
                            st.failed.push(name.clone());
                        }
                        _ => {}
                    }
                    match st.kind {
                        "spec" => {
                            out = format!(
                                "{}\n",
                                format_spec_line(status, &name, note, error.as_deref())
                            );
                        }
                        "tap" => {
                            if !st.header_done {
                                st.header_done = true;
                                out.push_str("TAP version 13\n");
                            }
                            out.push_str(&format_tap_line(
                                st.index,
                                status,
                                &name,
                                note,
                                error.as_deref(),
                            ));
                            out.push('\n');
                        }
                        "dot" => {
                            out = if matches!(status, ReportStatus::Ok) {
                                ".".to_owned()
                            } else {
                                "X".to_owned()
                            };
                        }
                        // junit/lcov：本仓无对应格式化器，输出为空（登记偏离）
                        _ => {}
                    }
                }
                "test:plan" if st.kind == "tap" => {
                    out = format!("1..{}\n", st.index);
                }
                "end" => {
                    st.ended = true;
                    match st.kind {
                        "tap" => out = format_tap_summary(&st.counts),
                        "spec" => out = format_spec_summary(&st.counts),
                        "dot" => out = format_dot_failed(&st.failed),
                        // lcov：compose 管道暂不输出（覆盖率数据通道为
                        // `aluka test --test-reporter=lcov`；登记偏离）
                        _ => {}
                    }
                }
                _ => {}
            }
        });
        out
    };
    if text.is_empty() {
        return Ok(Value::Undefined);
    }
    let text_val = Value::Object(vm.alloc_string(text.clone()));
    vm.invoke_callable(emit_fn, w, &[data_ev, text_val])?;
    Ok(Value::Object(vm.alloc_string(text)))
}

/// `end()`：出汇总块（tap `# tests` / spec `ℹ tests` / dot 失败清单）并触发
/// `finish` + `close` 事件（既有契约），返回汇总文本。
fn reporter_end(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let (w, w_id) = match crate::builtins::current_receiver() {
        Value::Object(r) => (Value::Object(r), r.0),
        _ => return Ok(Value::Undefined),
    };
    let text = {
        let mut out = String::new();
        REPORTER_STATES.with(|g| {
            if let Some(st) = g.borrow_mut().get_mut(&w_id) {
                if !st.ended {
                    st.ended = true;
                    match st.kind {
                        "tap" => out = format_tap_summary(&st.counts),
                        "spec" => out = format_spec_summary(&st.counts),
                        "dot" => out = format_dot_failed(&st.failed),
                        _ => {}
                    }
                }
            }
        });
        out
    };
    for event in ["finish", "close"] {
        let emit_fn = vm.get_property(w, "emit")?;
        let ev = ev_name(event, vm);
        vm.invoke_callable(emit_fn, w, &[ev])?;
    }
    Ok(Value::Object(vm.alloc_string(text)))
}

/// `pipe(dest)`：Node 可写流语义——返回 **destination**（对 `d.pipe(d) === d`
/// 同样成立；旧契约「返回自身」是 destination 特例）。
fn reporter_pipe(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(args
        .first()
        .copied()
        .unwrap_or_else(crate::builtins::current_receiver))
}

/// 事件名字符串分配。
fn ev_name(name: &str, vm: &mut Vm) -> Value {
    Value::Object(vm.alloc_string(name.to_owned()))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = reporter_ctor_spec;
        let _: crate::builtins::BuiltinHandler = reporter_ctor_tap;
        let _: crate::builtins::BuiltinHandler = reporter_ctor_dot;
        let _: crate::builtins::BuiltinHandler = reporter_ctor_junit;
        let _: crate::builtins::BuiltinHandler = reporter_write;
        let _: crate::builtins::BuiltinHandler = reporter_end;
        let _: crate::builtins::BuiltinHandler = reporter_pipe;
    }

    /// Node.js 22 LTS 标准实测样张。
    #[test]
    fn reporter_formatting_conform() {
        assert_eq!(
            format_spec_line(ReportStatus::Ok, "suite > case1", "", None),
            "ok    suite > case1"
        );
        assert_eq!(
            format_spec_line(
                ReportStatus::NotOk,
                "suite > case2",
                "",
                Some("aluka: assertion error: expected 2 but got 1")
            ),
            "not ok suite > case2\n       aluka: assertion error: expected 2 but got 1"
        );
        assert_eq!(
            format_spec_line(ReportStatus::Ok, "suite > skipme", "# SKIP", None),
            "ok    suite > skipme (SKIP)"
        );
        assert_eq!(
            format_spec_line(ReportStatus::NotOk, "todo1", "# TODO", Some("x")),
            "not ok todo1 (TODO)\n       x"
        );
    }

    /// TAP 行与汇总（Node.js 22 LTS 标准 `--test-reporter tap` 格式契约）。
    #[test]
    fn tap_lines_and_summaries_match_go_contract() {
        assert_eq!(
            format_tap_line(1, ReportStatus::Ok, "suite > case1", "", None),
            "ok 1 - suite > case1"
        );
        assert_eq!(
            format_tap_line(2, ReportStatus::Ok, "skipme", "# SKIP", None),
            "ok 2 - skipme SKIP"
        );
        let counts = ReportCounts {
            pass: 1,
            fail: 1,
            cancelled: 0,
            skipped: 1,
            todo: 0,
        };
        assert_eq!(
            format_tap_summary(&counts),
            "\n# tests 3\n# pass  1\n# fail  1\n# cancelled  0\n# skipped  1\n# todo  0"
        );
        assert_eq!(
            format_spec_summary(&counts),
            "\nℹ tests 3\nℹ pass  1\nℹ fail  1\nℹ cancelled  0\nℹ skipped  1\nℹ todo  0"
        );
    }

    /// dot 失败清单（Node.js 22 LTS 标准 格式契约）。
    #[test]
    fn dot_failed_block_matches_go_contract() {
        assert_eq!(format_dot_failed(&[]), "");
        assert_eq!(
            format_dot_failed(&["a > b".to_owned()]),
            "\nFailed tests:\n✖ a > b\n"
        );
    }

    /// 报告器形态分派（M5.4 切片一）：逐用例行与汇总块按 kind 组合既有纯函数。
    #[test]
    fn reporter_kind_dispatch_reuses_existing_formatters() {
        let cases = vec![
            ReportCase {
                name: "alpha".to_owned(),
                status: ReportStatus::Ok,
                note: String::new(),
                error: None,
            },
            ReportCase {
                name: "beta".to_owned(),
                status: ReportStatus::NotOk,
                note: String::new(),
                error: Some("boom".to_owned()),
            },
            ReportCase {
                name: "gamma".to_owned(),
                status: ReportStatus::Ok,
                note: "# SKIP".to_owned(),
                error: None,
            },
        ];
        assert_eq!(
            format_report_lines(&cases, ReporterKind::Spec),
            vec![
                "ok    alpha".to_owned(),
                "not ok beta\n       boom".to_owned(),
                "ok    gamma (SKIP)".to_owned(),
            ]
        );
        assert_eq!(
            format_report_lines(&cases, ReporterKind::Tap),
            vec![
                "ok 1 - alpha".to_owned(),
                "not ok 2 - beta\n  ---\n  message: boom\n  ...".to_owned(),
                "ok 3 - gamma SKIP".to_owned(),
            ]
        );
        // dot：逐用例无换行标记（拼接由调用方完成，对齐 Node dot 报告器）。
        assert_eq!(
            format_report_lines(&cases, ReporterKind::Dot),
            vec![".".to_owned(), "X".to_owned(), ".".to_owned()]
        );

        let counts = ReportCounts {
            pass: 2,
            fail: 1,
            cancelled: 0,
            skipped: 1,
            todo: 0,
        };
        assert_eq!(
            format_summary(&counts, &[], ReporterKind::Spec),
            format_spec_summary(&counts)
        );
        assert_eq!(
            format_summary(&counts, &[], ReporterKind::Tap),
            format_tap_summary(&counts)
        );
        assert_eq!(format_summary(&counts, &[], ReporterKind::Dot), "");
        assert_eq!(
            format_summary(&counts, &["beta".to_owned()], ReporterKind::Dot),
            format_dot_failed(&["beta".to_owned()])
        );
    }
}
