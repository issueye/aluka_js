//! `cluster` 内置模块（Phase 6）。
//!
//! 语义逐字对齐 Node.js 22 LTS 规范：
//! - 模块对象自带事件器表面（`on/once/emit/...`，`'fork'/'exit'/'message'`）；
//! - `isPrimary`/`isMaster`/`isWorker`：环境变量 `ALUKA_WORKER_ID` 标记 worker
//!   进程（worker 进程内另有 `worker = {id}`）；
//! - `workers`（id → Worker 实例）、`settings`（setupMaster/setupPrimary 写入）、
//!   `schedulingPolicy`/`SCHED_NONE`(1)/`SCHED_RR`(2)；
//! - `fork()`：复用 `child_process.fork` 派生当前可执行文件重跑当前脚本
//!   （Go 用 `os.Args[1]` 作脚本路径），并包一层 Worker 对象：child 的
//!   `'exit'`/`'message'` 事件转接到 Worker 与 cluster（exit code 硬编码 0，
//!   对齐 Go 的包装行为）；
//! - `disconnect([callback])`：对所有 worker 调 `destroy` 后调用回调；
//!   `Worker` 构造器返回普通对象（供 instanceof 表面）。

use crate::builtins::child_process::proc_common::{
    ns_attach, ns_emit, ns_push_listener, register_ns_emitter_handlers,
};
use crate::builtins::child_process::{SpawnOpts, fork_spawn};
use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;

/// `require("cluster")` / `require("node:cluster")` 模块导出。
pub const MODULE: ModuleDef = ModuleDef {
    name: "cluster",
    build,
};

// child 对象句柄 id → (worker id, worker 对象句柄 id)（事件转接用）；
// 线程局部：堆句柄仅本线程 Vm 有效。
thread_local! {
    static CHILD_TO_WORKER: RefCell<Option<HashMap<u32, (u64, u32)>>> =
        const { RefCell::new(None) };
}

fn with_child_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, (u64, u32)>) -> R,
{
    CHILD_TO_WORKER.with(|g| f(g.borrow_mut().get_or_insert_with(HashMap::new)))
}

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // isPrimary / isMaster / isWorker（ALUKA_WORKER_ID 标记 worker 进程）。
    let worker_id_env = std::env::var("ALUKA_WORKER_ID").unwrap_or_default();
    let is_primary = worker_id_env.is_empty();
    let _ = vm.set_property(Value::Object(obj), "isPrimary", Value::Boolean(is_primary));
    let _ = vm.set_property(Value::Object(obj), "isMaster", Value::Boolean(is_primary));
    let _ = vm.set_property(Value::Object(obj), "isWorker", Value::Boolean(!is_primary));

    // worker 进程内：cluster.worker = {id}。
    if !is_primary {
        let worker_obj = vm.alloc_ordinary();
        let id: f64 = worker_id_env.parse().unwrap_or(1.0);
        let _ = vm.set_property(Value::Object(worker_obj), "id", Value::Number(id));
        let _ = vm.set_property(Value::Object(obj), "worker", Value::Object(worker_obj));
    }
    // workers 表 / settings / schedulingPolicy 表面。
    let workers_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "workers", Value::Object(workers_obj));
    let settings_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(obj), "settings", Value::Object(settings_obj));
    let _ = vm.set_property(Value::Object(obj), "schedulingPolicy", Value::Number(1.0));
    let _ = vm.set_property(Value::Object(obj), "SCHED_NONE", Value::Number(1.0));
    let _ = vm.set_property(Value::Object(obj), "SCHED_RR", Value::Number(2.0));

    // 方法属性（分派经 module_of → "cluster.<method>"）。
    for method in [
        "fork",
        "setupMaster",
        "setupPrimary",
        "disconnect",
        "Worker",
        "on",
        "addListener",
        "once",
        "off",
        "removeListener",
        "removeAllListeners",
        "emit",
        "listenerCount",
    ] {
        let f = vm.alloc_native_fn(&format!("cluster.{method}"));
        set_module_prop(vm, obj, method, Value::Object(f))?;
    }
    register_ns_emitter_handlers(registry, "cluster");
    register_handler(registry, "cluster", "fork", cluster_fork);
    register_handler(registry, "cluster", "setupMaster", cluster_setup_master);
    register_handler(registry, "cluster", "setupPrimary", cluster_setup_master);
    register_handler(registry, "cluster", "disconnect", cluster_disconnect);
    register_handler(registry, "cluster", "Worker", cluster_worker_ctor);
    // 事件转接 wrapper（child 'exit'/'message' → worker + cluster 事件）。
    register_handler(registry, "cluster", "__workerExit", worker_exit_wrapper);
    register_handler(registry, "cluster", "__workerMsg", worker_msg_wrapper);
    // Worker 实例方法命名空间。
    register_ns_emitter_handlers(registry, "cluster:worker");
    register_handler(registry, "cluster:worker", "send", worker_send);
    register_handler(registry, "cluster:worker", "kill", worker_kill);
    register_handler(registry, "cluster:worker", "destroy", worker_destroy);
    register_handler(
        registry,
        "cluster:worker",
        "isConnected",
        worker_is_connected,
    );
    register_handler(registry, "cluster:worker", "isDead", worker_is_dead);
    Ok(obj)
}

/// `cluster.fork()`：child_process.fork 当前脚本 + Worker 包装。
fn cluster_fork(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    if !matches!(self_val, Value::Object(_)) {
        return Ok(self_val);
    }

    // worker id = 现有 workers 键数 + 1（Go len(workersObj.Keys())+1）。
    let worker_id = workers_count(vm, self_val) as u64 + 1;

    // 当前脚本路径（Go 取 os.Args[1]）。
    let script = std::env::args().nth(1).unwrap_or_default();

    // env：继承当前环境 + ALUKA_WORKER_ID 标记，用户传入 env 覆盖。
    let mut env_pairs: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.to_string_lossy().to_string(),
            )
        })
        .collect();
    env_pairs.push(("ALUKA_WORKER_ID".to_owned(), worker_id.to_string()));
    if let Some(user_env) = args.first().copied() {
        if let Value::Object(_) = user_env {
            for (k, v) in vm.own_properties(user_env) {
                env_pairs.push((k, vm.format_value(v)));
            }
        }
    }
    let opts = SpawnOpts {
        silent: Some(false),
        cwd: String::new(),
        windows_hide: cfg!(windows),
        env: Some(env_pairs),
    };

    let child_val = fork_spawn(vm, script, Vec::new(), opts)?;
    let Value::Object(child_ref) = child_val else {
        return Ok(child_val);
    };

    // Worker 对象（事件器语义）+ process 引用。
    let worker = vm.alloc_ordinary();
    ns_attach(
        vm,
        worker,
        "cluster:worker",
        &[
            "on",
            "once",
            "off",
            "emit",
            "listenerCount",
            "send",
            "kill",
            "destroy",
            "isConnected",
            "isDead",
        ],
    );
    let _ = vm.set_property(Value::Object(worker), "id", Value::Number(worker_id as f64));
    let _ = vm.set_property(Value::Object(worker), "process", child_val);
    with_child_map(|m| {
        m.insert(child_ref.0, (worker_id, worker.0));
    });

    // child 'exit'/'message' 事件转接（注册 Rust 侧监听器）。
    attach_child_wrapper(vm, child_ref.0, "exit", "cluster.__workerExit")?;
    attach_child_wrapper(vm, child_ref.0, "message", "cluster.__workerMsg")?;

    // workers[id] = worker；同步触发 cluster 'fork'。
    let workers_val = vm.get_property(self_val, "workers")?;
    let _ = vm.set_property(workers_val, &worker_id.to_string(), Value::Object(worker));
    ns_emit(vm, self_val, "fork", &[Value::Object(worker)])?;
    Ok(Value::Object(worker))
}

/// child 事件上的内部转接监听器（原生函数注册进实例事件器监听器表）。
fn attach_child_wrapper(
    vm: &mut Vm,
    child_id: u32,
    event: &str,
    native_name: &str,
) -> Result<(), VmError> {
    let wrapper = vm.alloc_native_fn(native_name);
    ns_push_listener(child_id, event, Value::Object(wrapper));
    Ok(())
}

/// child 'exit' 转接：清理 workers 表 → cluster 'exit'(worker, 0, null) →
/// worker 'exit'(0, null)（Go 包装的硬编码退出码）。
fn worker_exit_wrapper(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let child = current_receiver();
    let Value::Object(child_ref) = child else {
        return Ok(Value::Undefined);
    };
    let entry = with_child_map(|m| m.remove(&child_ref.0));
    let Some((worker_id, worker_ref)) = entry else {
        return Ok(Value::Undefined);
    };
    // cluster 模块对象：从分派表反查（模块单例）。
    let Some(module_ref) = vm.builtin_registry.module("cluster") else {
        return Ok(Value::Undefined);
    };
    let module_val = Value::Object(module_ref);
    let workers_val = vm.get_property(module_val, "workers")?;
    if let Value::Object(_) = workers_val {
        vm.delete_property(workers_val, &worker_id.to_string());
    }
    let worker_val = Value::Object(ObjectRef(worker_ref));
    ns_emit(
        vm,
        module_val,
        "exit",
        &[worker_val, Value::Number(0.0), Value::Null],
    )?;
    ns_emit(vm, worker_val, "exit", &[Value::Number(0.0), Value::Null])?;
    Ok(Value::Undefined)
}

/// child 'message' 转接：worker 'message'(msg) → cluster 'message'(msg, worker)。
fn worker_msg_wrapper(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let child = current_receiver();
    let Value::Object(child_ref) = child else {
        return Ok(Value::Undefined);
    };
    let entry = with_child_map(|m| m.get(&child_ref.0).copied());
    let Some((_, worker_ref)) = entry else {
        return Ok(Value::Undefined);
    };
    let Some(module_ref) = vm.builtin_registry.module("cluster") else {
        return Ok(Value::Undefined);
    };
    let worker_val = Value::Object(ObjectRef(worker_ref));
    if let Some(msg) = args.first().copied() {
        ns_emit(vm, worker_val, "message", &[msg])?;
        ns_emit(vm, Value::Object(module_ref), "message", &[msg, worker_val])?;
    }
    Ok(Value::Undefined)
}

/// workers 对象的键数（worker id 个数）。
fn workers_count(vm: &mut Vm, module_val: Value) -> usize {
    let Ok(workers_val) = vm.get_property(module_val, "workers") else {
        return 0;
    };
    vm.own_properties(workers_val).len()
}

/// `cluster.setupMaster([settings])` / `setupPrimary`：写入 settings 的既定键。
fn cluster_setup_master(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let Some(opts) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    if let Value::Object(_) = opts {
        let settings = vm.get_property(self_val, "settings")?;
        for k in [
            "exec",
            "execArgv",
            "args",
            "silent",
            "cwd",
            "serialization",
            "stdio",
        ] {
            if let Ok(v) = vm.get_property(opts, k) {
                if !matches!(v, Value::Undefined) {
                    let _ = vm.set_property(settings, k, v);
                }
            }
        }
    }
    Ok(Value::Undefined)
}

/// `cluster.disconnect([callback])`：逐 worker destroy 后调用回调。
fn cluster_disconnect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let self_val = current_receiver();
    let workers_val = vm.get_property(self_val, "workers")?;
    let worker_vals: Vec<Value> = vm
        .own_properties(workers_val)
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    for w in worker_vals {
        if let Ok(destroy_fn) = vm.get_property(w, "destroy") {
            vm.invoke_callable(destroy_fn, w, &[])?;
        }
    }
    if let Some(cb) = args.first().copied() {
        if is_callable(vm, cb) {
            vm.invoke_callable(cb, Value::Undefined, &[])?;
        }
    }
    Ok(Value::Undefined)
}

/// `cluster.Worker` 构造器：返回普通对象（Go 同款表面）。
fn cluster_worker_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(vm.alloc_ordinary()))
}

/// worker `send()`：委托简化，恒 true（Go 同款）。
fn worker_send(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(true))
}

/// worker `kill()`：简化恒 true（Go 同款）。
fn worker_kill(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(true))
}

/// worker `destroy()`。
fn worker_destroy(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// worker `isConnected()`：恒 true（Go 同款）。
fn worker_is_connected(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(true))
}

/// worker `isDead()`：恒 false（Go 同款）。
fn worker_is_dead(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(false))
}

/// 判断值是否为可调用堆对象。
fn is_callable(vm: &Vm, val: Value) -> bool {
    let Value::Object(r) = val else {
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
