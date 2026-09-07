//! `perf_hooks` 内置模块（Phase 3）：Performance / User Timing API。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodediag.NewPerfHooks`）：
//! - `performance.now()`：基于启动原点的单调时钟（毫秒浮点数）；
//! - `performance.timeOrigin`：虚拟机启动时间基准（Unix 毫秒）；
//! - `performance.mark` / `measure` / `getEntries*` / `clearMarks`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// 单条性能记录。
#[derive(Debug, Clone)]
struct PerfEntry {
    name: String,
    entry_type: String,
    start_time: f64,
    duration: f64,
}

static PERF_STATE: Mutex<Option<PerfState>> = Mutex::new(None);

struct PerfState {
    start_instant: Instant,
    time_origin: f64,
    entries: Vec<PerfEntry>,
    marks: std::collections::HashMap<String, f64>,
}

fn init_state() {
    let mut guard = PERF_STATE.lock().unwrap();
    if guard.is_none() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        *guard = Some(PerfState {
            start_instant: Instant::now(),
            time_origin: now_ms,
            entries: Vec::new(),
            marks: std::collections::HashMap::new(),
        });
    }
}

/// `require("perf_hooks")` / `require("node:perf_hooks")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "perf_hooks",
    build,
};

/// `performance` 单例子模块（供直调方法分派）。
pub const PERFORMANCE_MODULE: ModuleDef = ModuleDef {
    name: "performance",
    build: build_performance,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    init_state();
    let obj = vm.alloc_ordinary();
    let perf_obj = vm.alloc_ordinary();

    set_module_prop(vm, obj, "performance", Value::Object(perf_obj))?;

    let time_origin = {
        let guard = PERF_STATE.lock().unwrap();
        guard.as_ref().map(|s| s.time_origin).unwrap_or(0.0)
    };
    set_module_prop(vm, perf_obj, "timeOrigin", Value::Number(time_origin))?;

    for method in [
        "now",
        "mark",
        "measure",
        "getEntries",
        "getEntriesByType",
        "getEntriesByName",
        "clearMarks",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("performance.{method}"));
        set_module_prop(vm, perf_obj, method, Value::Object(fn_ref))?;
    }

    register_handler(registry, "performance", "now", now);
    register_handler(registry, "performance", "mark", mark);
    register_handler(registry, "performance", "measure", measure);
    register_handler(registry, "performance", "getEntries", get_entries);
    register_handler(
        registry,
        "performance",
        "getEntriesByType",
        get_entries_by_type,
    );
    register_handler(
        registry,
        "performance",
        "getEntriesByName",
        get_entries_by_name,
    );
    register_handler(registry, "performance", "clearMarks", clear_marks);

    Ok(obj)
}

fn build_performance(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let perf_mod = registry.module("perf_hooks").ok_or_else(|| {
        let msg = vm.alloc_string("perf_hooks 模块尚未初始化".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    let val = vm.get_property(Value::Object(perf_mod), "performance")?;
    match val {
        Value::Object(r) => Ok(r),
        _ => Err(VmError::Thrown(Value::Object(
            vm.alloc_string("performance 属性缺失".to_owned()),
        ))),
    }
}

/// `performance.now()`
fn now(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let guard = PERF_STATE.lock().unwrap();
    let elapsed = guard
        .as_ref()
        .map(|s| s.start_instant.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    Ok(Value::Number(elapsed))
}

/// `performance.mark(name)`
fn mark(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "default".to_owned());

    let mut guard = PERF_STATE.lock().unwrap();
    let state = guard.as_mut().unwrap();
    let start_time = state.start_instant.elapsed().as_secs_f64() * 1000.0;

    state.marks.insert(name.clone(), start_time);
    let entry = PerfEntry {
        name: name.clone(),
        entry_type: "mark".to_owned(),
        start_time,
        duration: 0.0,
    };
    state.entries.push(entry.clone());

    Ok(Value::Object(perf_entry_to_obj(vm, &entry)))
}

/// `performance.measure(name, [startMark, endMark])`
fn measure(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "default".to_owned());

    let start_name = args.get(1).map(|v| vm.format_value(*v));
    let end_name = args.get(2).map(|v| vm.format_value(*v));

    let mut guard = PERF_STATE.lock().unwrap();
    let state = guard.as_mut().unwrap();
    let now = state.start_instant.elapsed().as_secs_f64() * 1000.0;

    let start_time = if let Some(sn) = start_name {
        state.marks.get(&sn).copied().unwrap_or(0.0)
    } else {
        0.0
    };

    let end_time = if let Some(en) = end_name {
        state.marks.get(&en).copied().unwrap_or(now)
    } else {
        now
    };

    let duration = (end_time - start_time).max(0.0);
    let entry = PerfEntry {
        name: name.clone(),
        entry_type: "measure".to_owned(),
        start_time,
        duration,
    };
    state.entries.push(entry.clone());

    Ok(Value::Object(perf_entry_to_obj(vm, &entry)))
}

/// `performance.getEntries()`
fn get_entries(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let guard = PERF_STATE.lock().unwrap();
    let entries = guard
        .as_ref()
        .map(|s| s.entries.clone())
        .unwrap_or_default();
    let list: Vec<Value> = entries
        .iter()
        .map(|e| Value::Object(perf_entry_to_obj(vm, e)))
        .collect();
    Ok(Value::Object(vm.alloc_array(list)))
}

/// `performance.getEntriesByType(type)`
fn get_entries_by_type(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let typ = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let guard = PERF_STATE.lock().unwrap();
    let entries = guard
        .as_ref()
        .map(|s| s.entries.clone())
        .unwrap_or_default();
    let list: Vec<Value> = entries
        .iter()
        .filter(|e| e.entry_type == typ)
        .map(|e| Value::Object(perf_entry_to_obj(vm, e)))
        .collect();
    Ok(Value::Object(vm.alloc_array(list)))
}

/// `performance.getEntriesByName(name)`
fn get_entries_by_name(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let name = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let guard = PERF_STATE.lock().unwrap();
    let entries = guard
        .as_ref()
        .map(|s| s.entries.clone())
        .unwrap_or_default();
    let list: Vec<Value> = entries
        .iter()
        .filter(|e| e.name == name)
        .map(|e| Value::Object(perf_entry_to_obj(vm, e)))
        .collect();
    Ok(Value::Object(vm.alloc_array(list)))
}

/// `performance.clearMarks([name])`
fn clear_marks(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    init_state();
    let target = args.first().map(|v| vm.format_value(*v));
    let mut guard = PERF_STATE.lock().unwrap();
    let state = guard.as_mut().unwrap();
    if let Some(name) = target {
        state.marks.remove(&name);
        state
            .entries
            .retain(|e| !(e.entry_type == "mark" && e.name == name));
    } else {
        state.marks.clear();
        state.entries.retain(|e| e.entry_type != "mark");
    }
    Ok(Value::Undefined)
}

fn perf_entry_to_obj(vm: &mut Vm, e: &PerfEntry) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    let name_ref = vm.alloc_string(e.name.clone());
    let type_ref = vm.alloc_string(e.entry_type.clone());
    let _ = vm.set_property(Value::Object(obj), "name", Value::Object(name_ref));
    let _ = vm.set_property(Value::Object(obj), "entryType", Value::Object(type_ref));
    let _ = vm.set_property(Value::Object(obj), "startTime", Value::Number(e.start_time));
    let _ = vm.set_property(Value::Object(obj), "duration", Value::Number(e.duration));
    obj
}
