//! `test/reporters` 内置模块（Phase 8）：测试报告器表面与报告行格式化。
//!
//! 移植 Node.js 22 LTS 标准（`nodetest/test_reporters.go` 模块面 + `cmd/aluka`
//! `printTestLine`/汇总格式）：
//! - 模块面：`dot`/`junit`/`spec`/`tap` 五个可构造报告器（`new X()` 得
//!   到可写流实例：`write`/`end`/`on`/`pipe`；`end` 触发 `finish`+`close`）；
//!   `lcov` 为预构造实例（Node 22：不可 new）；
//! - 格式化面：spec/tap 报告行与汇总的纯函数移植（Go CLI 契约，
//!   `aluka test` 子命令生产路径实际调用——报告格式源自本仓 CLI
//!   `printTestLine`/汇总格式，**不声称与 `node --test` 逐字一致**；
//!   dot 报告器的逐用例标记沿用 Node dot 报告器 `.`/`X` 形态）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

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

/// 构建 test/reporters 模块导出对象（对齐 Go `NewTestReporters`）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let m = vm.alloc_ordinary();

    // 类报告器：dot / junit / spec / tap（可 new；实例为可写流）。
    for name in ["dot", "junit", "spec", "tap"] {
        let ctor = vm.alloc_native_fn(&format!("test/reporters.{name}"));
        set_module_prop(vm, m, name, Value::Object(ctor))?;
        register_handler(registry, "test/reporters", name, reporter_ctor);
    }

    // lcov：Node 22 导出为预构造实例（object），不可 new。
    let lcov = new_reporter_stream(vm);
    set_module_prop(vm, m, "lcov", Value::Object(lcov))?;

    // 流方法分派键（实例方法按名称分派，键在 build 期一次性登记）。
    register_handler(registry, "test/reporters:stream", "write", reporter_write);
    register_handler(registry, "test/reporters:stream", "end", reporter_end);
    register_handler(registry, "test/reporters:stream", "pipe", reporter_pipe);

    Ok(m)
}

/// 报告器构造调用：`new X()` / `X()` 均返回可写流实例。
fn reporter_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(new_reporter_stream(vm)))
}

/// 报告器底层可写流：发射器实例 + `_builtinNs` 分派（on/emit 等复用
/// `events:instance` 处理器），`write`（转发 `data` 事件）/`end`
/// `pipe`（返回自身）——对齐 Go `newReporterStream`。
fn new_reporter_stream(vm: &mut Vm) -> ObjectRef {
    let w = crate::builtins::events::create_emitter_instance(vm);
    let ns_val = Value::Object(vm.alloc_string("test/reporters:stream".to_owned()));
    let _ = vm.set_property(Value::Object(w), "_builtinNs", ns_val);
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
    }
    let write_fn = vm.alloc_native_fn("test/reporters:stream.write");
    let _ = vm.set_property(Value::Object(w), "write", Value::Object(write_fn));
    let end_fn = vm.alloc_native_fn("test/reporters:stream.end");
    let _ = vm.set_property(Value::Object(w), "end", Value::Object(end_fn));
    let pipe_fn = vm.alloc_native_fn("test/reporters:stream.pipe");
    let _ = vm.set_property(Value::Object(w), "pipe", Value::Object(pipe_fn));
    w
}

/// `write(chunk)`：把 chunk 以 `data` 事件转发（Node 可写流契约；此前直接
/// 吞掉数据，报告器实例因此无任何可见输出），返回值恒为 `true`（背压已接受）。
fn reporter_write(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let w = crate::builtins::current_receiver();
    let chunk = args.first().copied().unwrap_or(Value::Undefined);
    let emit_fn = vm.get_property(w, "emit")?;
    let ev_name = Value::Object(vm.alloc_string("data".to_owned()));
    vm.invoke_callable(emit_fn, w, &[ev_name, chunk])?;
    Ok(Value::Boolean(true))
}

/// `end()`：触发 `finish` + `close` 事件（对齐 Go）。
fn reporter_end(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let w = crate::builtins::current_receiver();
    for event in ["finish", "close"] {
        let emit_fn = vm.get_property(w, "emit")?;
        let ev_name = Value::Object(vm.alloc_string(event.to_owned()));
        vm.invoke_callable(emit_fn, w, &[ev_name])?;
    }
    Ok(Value::Undefined)
}

/// `pipe(dest)`：返回自身（对齐 Go 最小语义）。
fn reporter_pipe(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(crate::builtins::current_receiver())
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = reporter_ctor;
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
