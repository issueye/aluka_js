//! `sqlite` 内置模块（Phase 7）：Node 22 `node:sqlite` 原生 `DatabaseSync`。
//!
//! 语义严格对齐 Node.js 22 LTS 规范：
//! - `new DatabaseSync(path)`（别名 `Database`）打开数据库（`:memory:` 或文件路径）；
//! - `db.exec(sql)` 执行多语句；`db.prepare(sql) -> StatementSync`；
//!   `db.transaction(fn)` 返回事务包装函数；`db.close()`；`isOpen` 数据属性；
//! - `StatementSync.run/get/all/iterate/setReadBigInts/columns`，支持位置与命名参数；
//! - 值类型：`null` / `number` / `bigint`（`setReadBigInts`）/ `string` / `Buffer`；
//! - 错误文本复刻 modernc.org/sqlite 驱动形态（`errstr: errmsg (extcode)`）。
//!
//! # FFI 边界说明（AGENTS.md 例外条款）
//!
//! 本模块是工作区唯一获准的 C 依赖：`rusqlite` + `bundled` feature 把 SQLite C
//! amalgamation 静态编进二进制（单文件、零运行时依赖约束仍成立）。依据
//! AGENTS.md「GC 分配器、JIT 机器码发射、FFI 边界可显式解禁」的 FFI 边界例外：
//! 本文件本身**不含任何 `unsafe`**——rusqlite 对 `libsqlite3-sys` 的内部 unsafe
//! 调用由 rusqlite 上游自行维护并论证，本模块只经其安全 API（`Connection` /
//! `Statement` / `ValueRef`）访问 SQLite。

use crate::builtins::{
    BuiltinRegistry, ModuleDef, current_receiver, register_handler, set_module_prop,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, Statement};
use std::collections::HashMap;
use std::sync::Mutex;

/// `require("sqlite")` / `require("node:sqlite")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "sqlite",
    build,
};

/// JS 绑定值转译后的驱动参数（对齐 Go `sqliteParamToDriver` 的五种落点）。
#[derive(Debug, Clone)]
enum SqlParam {
    /// `null` / `undefined`
    Null,
    /// 整数（含可转 `i64` 的 bigint）
    Int(i64),
    /// 浮点数
    Real(f64),
    /// 文本
    Text(String),
    /// 二进制（Buffer 实例）
    Blob(Vec<u8>),
}

impl SqlParam {
    /// 转为 rusqlite 可绑定值。
    fn to_sql_value(&self) -> SqlValue {
        match self {
            Self::Null => SqlValue::Null,
            Self::Int(i) => SqlValue::Integer(*i),
            Self::Real(f) => SqlValue::Real(*f),
            Self::Text(s) => SqlValue::Text(s.clone()),
            Self::Blob(b) => SqlValue::Blob(b.clone()),
        }
    }
}

/// 参数绑定计划：位置参数或命名参数。
#[derive(Debug, Clone)]
enum BindPlan {
    /// 位置参数列表
    Positional(Vec<SqlParam>),
    /// 命名参数（键名不含 `:` 前缀）
    Named(Vec<(String, SqlParam)>),
}

/// `DatabaseSync` 实例的连接状态（键为实例堆句柄索引）。
struct DbEntry {
    /// SQLite 连接（单连接模型：BEGIN/COMMIT 同连接同步语义）
    conn: Connection,
}

/// `StatementSync` 实例状态（语句按 SQL 文本在执行期重编译，语义等价预编译）。
struct StmtEntry {
    /// 所属数据库实例句柄索引
    db_id: u32,
    /// 源 SQL 文本（`sourceSQL` 属性）
    sql: String,
    /// INTEGER 列读取为 bigint（`setReadBigInts`）
    read_big_ints: bool,
}

/// `iterate()` 物化后的行集（`next()` 逐行弹出）。
struct IterEntry {
    /// 已物化的行（列名 → 所有权值）
    rows: Vec<Vec<(String, SqlValue)>>,
    /// 下一条行下标
    pos: usize,
    /// bigint 读取开关（登记时从语句快照）
    read_big_ints: bool,
}

/// 事务包装函数捕获的上下文（键为包装函数堆句柄索引）。
struct TxEntry {
    /// 所属数据库实例句柄索引
    db_id: u32,
    /// 事务回调（JS 函数值）
    callback: Value,
}

static DBS: Mutex<Option<HashMap<u32, DbEntry>>> = Mutex::new(None);
static STMTS: Mutex<Option<HashMap<u32, StmtEntry>>> = Mutex::new(None);
static ITERS: Mutex<Option<HashMap<u32, IterEntry>>> = Mutex::new(None);
static TXNS: Mutex<Option<HashMap<u32, TxEntry>>> = Mutex::new(None);

/// 在状态表上执行闭包（惰性初始化；各表独立加锁、不嵌套，避免锁序问题）。
fn with_map<T, F, R>(m: &Mutex<Option<HashMap<u32, T>>>, f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, T>) -> R,
{
    let mut guard = m.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// 构建 `sqlite` 模块对象：`DatabaseSync`（别名 `Database`）构造器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let ctor = vm.alloc_native_fn("sqlite.DatabaseSync");
    set_module_prop(vm, obj, "DatabaseSync", Value::Object(ctor))?;
    // bun:sqlite / better-sqlite3 兼容别名（同一函数值，双属性）。
    set_module_prop(vm, obj, "Database", Value::Object(ctor))?;
    register_handler(registry, "sqlite", "DatabaseSync", database_sync_ctor);
    register_handler(registry, "sqlite", "Database", database_sync_ctor);
    register_handler(registry, "sqlite:db", "exec", db_exec);
    register_handler(registry, "sqlite:db", "prepare", db_prepare);
    register_handler(registry, "sqlite:db", "close", db_close);
    register_handler(registry, "sqlite:db", "transaction", db_transaction);
    register_handler(registry, "sqlite:txn", "call", txn_call);
    register_handler(registry, "sqlite:stmt", "run", stmt_run);
    register_handler(registry, "sqlite:stmt", "get", stmt_get);
    register_handler(registry, "sqlite:stmt", "all", stmt_all);
    register_handler(registry, "sqlite:stmt", "iterate", stmt_iterate);
    register_handler(
        registry,
        "sqlite:stmt",
        "setReadBigInts",
        stmt_set_read_big_ints,
    );
    register_handler(registry, "sqlite:stmt", "columns", stmt_columns);
    register_handler(registry, "sqlite:iter", "next", iter_next);
    Ok(obj)
}

/// `new DatabaseSync(path)`：打开数据库连接并返回实例。
fn database_sync_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let conn = Connection::open(&path).map_err(|e| {
        sqlite_throw(
            vm,
            &format!("node:sqlite: cannot open \"{path}\": {}", fmt_open_err(&e)),
        )
    })?;

    let obj = vm.alloc_ordinary();
    let ns = ns_value(vm, "sqlite:db");
    set_module_prop(vm, obj, "_builtinNs", ns)?;
    for method in ["exec", "prepare", "close", "transaction"] {
        let fn_ref = vm.alloc_native_fn(&format!("sqlite:db.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    // `isOpen` 以数据属性维护（open 时 true，close 后置 false）。
    set_module_prop(vm, obj, "isOpen", Value::Boolean(true))?;

    let id = obj.0;
    with_map(&DBS, |m| {
        m.insert(id, DbEntry { conn });
    });
    Ok(Value::Object(obj))
}

/// `db.exec(sql)`：执行一段（可含多语句的）SQL，无返回行。
fn db_exec(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(sql) = args.first().map(|v| vm.format_value(*v)) else {
        return Err(sqlite_throw(vm, "node:sqlite: exec requires SQL string"));
    };
    let id = require_db_id(vm)?;
    let outcome = with_map(&DBS, |m| {
        let Some(entry) = m.get_mut(&id) else {
            return Err("node:sqlite: sql: database is closed".to_owned());
        };
        entry
            .conn
            .execute_batch(&sql)
            .map_err(|e| format!("node:sqlite: {}", fmt_driver_err(&e)))
    });
    match outcome {
        Ok(()) => Ok(Value::Undefined),
        Err(msg) => Err(sqlite_throw(vm, &msg)),
    }
}

/// `db.prepare(sql)`：编译语句并返回 `StatementSync` 实例。
fn db_prepare(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(sql) = args.first().map(|v| vm.format_value(*v)) else {
        return Err(sqlite_throw(vm, "node:sqlite: prepare requires SQL string"));
    };
    let db_id = require_db_id(vm)?;
    // prepare 期即校验语法（对齐 Go：非法 SQL 在 prepare 时报错）。
    let prep = with_map(&DBS, |m| {
        let Some(entry) = m.get_mut(&db_id) else {
            return Err("node:sqlite: sql: database is closed".to_owned());
        };
        entry
            .conn
            .prepare(&sql)
            .map(|_| ())
            .map_err(|e| format!("node:sqlite: {}", fmt_driver_err(&e)))
    });
    if let Err(msg) = prep {
        return Err(sqlite_throw(vm, &msg));
    }

    let obj = vm.alloc_ordinary();
    let ns = ns_value(vm, "sqlite:stmt");
    set_module_prop(vm, obj, "_builtinNs", ns)?;
    let source = Value::Object(vm.alloc_string(sql.clone()));
    set_module_prop(vm, obj, "sourceSQL", source)?;
    for method in ["run", "get", "all", "iterate", "setReadBigInts", "columns"] {
        let fn_ref = vm.alloc_native_fn(&format!("sqlite:stmt.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    let id = obj.0;
    with_map(&STMTS, |m| {
        m.insert(
            id,
            StmtEntry {
                db_id,
                sql,
                read_big_ints: false,
            },
        );
    });
    Ok(Value::Object(obj))
}

/// `db.close()`：关闭连接、清理语句并把 `isOpen` 置 false。
fn db_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let id = r.0;
    with_map(&STMTS, |m| {
        m.retain(|_, e| e.db_id != id);
    });
    with_map(&DBS, |m| {
        m.remove(&id);
    });
    set_module_prop(vm, r, "isOpen", Value::Boolean(false))?;
    Ok(Value::Undefined)
}

/// `db.transaction(fn)`：返回事务包装函数（BEGIN → fn → COMMIT，异常则 ROLLBACK）。
fn db_transaction(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let db_id = require_db_id(vm)?;
    let Some(callback) = args.first().copied().filter(|v| is_callable_value(vm, *v)) else {
        return Err(sqlite_throw(
            vm,
            "node:sqlite: transaction requires function",
        ));
    };
    let wrapper = vm.alloc_native_fn("sqlite:txn.call");
    let wid = wrapper.0;
    with_map(&TXNS, |m| {
        m.insert(wid, TxEntry { db_id, callback });
    });
    Ok(Value::Object(wrapper))
}

/// 事务包装函数调用：BEGIN → 回调 → COMMIT / ROLLBACK。
fn txn_call(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let found = with_map(&TXNS, |m| m.get(&r.0).map(|e| (e.db_id, e.callback)));
    let Some((db_id, callback)) = found else {
        return Ok(Value::Undefined);
    };
    let begun = with_map(&DBS, |m| {
        let Some(entry) = m.get_mut(&db_id) else {
            return Err("node:sqlite: sql: database is closed".to_owned());
        };
        entry
            .conn
            .execute_batch("BEGIN")
            .map_err(|e| format!("node:sqlite: begin transaction: {}", fmt_driver_err(&e)))
    });
    if let Err(msg) = begun {
        return Err(sqlite_throw(vm, &msg));
    }
    match vm.invoke_callable(callback, Value::Undefined, args) {
        Ok(ret) => {
            let committed = with_map(&DBS, |m| {
                let Some(entry) = m.get_mut(&db_id) else {
                    return Err("node:sqlite: sql: database is closed".to_owned());
                };
                entry
                    .conn
                    .execute_batch("COMMIT")
                    .map_err(|e| format!("node:sqlite: commit transaction: {}", fmt_driver_err(&e)))
            });
            match committed {
                Ok(()) => Ok(ret),
                Err(msg) => {
                    rollback_quiet(db_id);
                    Err(sqlite_throw(vm, &msg))
                }
            }
        }
        Err(e) => {
            rollback_quiet(db_id);
            Err(e)
        }
    }
}

/// 事务失败时的静默 ROLLBACK。
fn rollback_quiet(db_id: u32) {
    with_map(&DBS, |m| {
        if let Some(entry) = m.get_mut(&db_id) {
            let _ = entry.conn.execute_batch("ROLLBACK");
        }
    });
}

/// 取当前接收者（DatabaseSync 实例）的句柄索引。
fn require_db_id(vm: &mut Vm) -> Result<u32, VmError> {
    let receiver = current_receiver();
    match receiver {
        Value::Object(r) => Ok(r.0),
        _ => Err(make_error(
            vm,
            "Error",
            "node:sqlite: receiver is not a DatabaseSync",
        )),
    }
}

/// `stmt.run(...params)`：执行写语句，返回 `{ changes, lastInsertRowid }`。
fn stmt_run(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let plan = to_bind_plan(vm, args)?;
    let key = current_stmt_key(vm)?;
    let outcome = exec_on_stmt(&key, run_inner, &plan);
    let (changes, last_id) = map_stmt_result(vm, outcome)?;
    let obj = vm.alloc_ordinary();
    let changes_v = Value::Number(changes as f64);
    set_module_prop(vm, obj, "changes", changes_v)?;
    let last_v = Value::Number(last_id as f64);
    set_module_prop(vm, obj, "lastInsertRowid", last_v)?;
    Ok(Value::Object(obj))
}

/// `run` 内核：raw_execute 取 changes，语句结束后取连接级 last rowid。
fn run_inner(conn: &mut Connection, sql: &str, plan: &BindPlan) -> Result<(i64, i64), String> {
    let changes = {
        let mut stmt = conn.prepare(sql).map_err(fmt_driver_err_owned)?;
        bind_plan(&mut stmt, plan)?;
        stmt.raw_execute().map_err(fmt_driver_err_owned)? as i64
    };
    let last_id = conn.last_insert_rowid();
    Ok((changes, last_id))
}

/// `stmt.get(...params)`：取第一行（无行返回 `undefined`）。
fn stmt_get(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let plan = to_bind_plan(vm, args)?;
    let key = current_stmt_key(vm)?;
    let read_big_ints = stmt_read_big_ints(&key).unwrap_or(false);
    let outcome = exec_on_stmt(&key, get_inner, &plan);
    match map_stmt_result(vm, outcome)? {
        Some(cells) => {
            let obj = row_to_js(vm, read_big_ints, &cells)?;
            Ok(Value::Object(obj))
        }
        None => Ok(Value::Undefined),
    }
}

/// `get` 内核：执行查询并物化首行。
fn get_inner(
    conn: &mut Connection,
    sql: &str,
    plan: &BindPlan,
) -> Result<Option<Vec<(String, SqlValue)>>, String> {
    let mut stmt = conn.prepare(sql).map_err(fmt_driver_err_owned)?;
    bind_plan(&mut stmt, plan)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut rows = stmt.raw_query();
    match rows.next().map_err(fmt_driver_err_owned)? {
        Some(row) => {
            let mut cells = Vec::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let v = row.get_ref(i).map_err(fmt_driver_err_owned)?;
                cells.push((name.clone(), owned_value(v)));
            }
            Ok(Some(cells))
        }
        None => Ok(None),
    }
}

/// `stmt.all(...params)`：取全部行为对象数组。
fn stmt_all(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let plan = to_bind_plan(vm, args)?;
    let key = current_stmt_key(vm)?;
    let read_big_ints = stmt_read_big_ints(&key).unwrap_or(false);
    let outcome = exec_on_stmt(&key, rows_inner, &plan);
    let rows = map_stmt_result(vm, outcome)?;
    let mut elems: Vec<Value> = Vec::with_capacity(rows.len());
    for cells in &rows {
        elems.push(Value::Object(row_to_js(vm, read_big_ints, cells)?));
    }
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `stmt.iterate(...params)`：物化行集并返回 `next()` 同步迭代器。
fn stmt_iterate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let plan = to_bind_plan(vm, args)?;
    let key = current_stmt_key(vm)?;
    let read_big_ints = stmt_read_big_ints(&key).unwrap_or(false);
    let outcome = exec_on_stmt(&key, rows_inner, &plan);
    let rows = map_stmt_result(vm, outcome)?;

    let iter = vm.alloc_ordinary();
    let ns = ns_value(vm, "sqlite:iter");
    set_module_prop(vm, iter, "_builtinNs", ns)?;
    let next_fn = vm.alloc_native_fn("sqlite:iter.next");
    set_module_prop(vm, iter, "next", Value::Object(next_fn))?;
    let id = iter.0;
    with_map(&ITERS, |m| {
        m.insert(
            id,
            IterEntry {
                rows,
                pos: 0,
                read_big_ints,
            },
        );
    });
    Ok(Value::Object(iter))
}

/// `all`/`iterate` 共用内核：执行查询并物化全部行。
fn rows_inner(
    conn: &mut Connection,
    sql: &str,
    plan: &BindPlan,
) -> Result<Vec<Vec<(String, SqlValue)>>, String> {
    let mut stmt = conn.prepare(sql).map_err(fmt_driver_err_owned)?;
    bind_plan(&mut stmt, plan)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut rows = stmt.raw_query();
    let mut out: Vec<Vec<(String, SqlValue)>> = Vec::new();
    loop {
        match rows.next().map_err(fmt_driver_err_owned)? {
            Some(row) => {
                let mut cells = Vec::with_capacity(names.len());
                for (i, name) in names.iter().enumerate() {
                    let v = row.get_ref(i).map_err(fmt_driver_err_owned)?;
                    cells.push((name.clone(), owned_value(v)));
                }
                out.push(cells);
            }
            None => return Ok(out),
        }
    }
}

/// `iter.next()`：`{ done: false, value: 行对象 }` 或 `{ done: true }`。
fn iter_next(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let (next, read_big_ints) = with_map(&ITERS, |m| {
        let Some(entry) = m.get_mut(&r.0) else {
            return (None, false);
        };
        let flag = entry.read_big_ints;
        if entry.pos >= entry.rows.len() {
            return (None, flag);
        }
        let cells = entry.rows[entry.pos].clone();
        entry.pos += 1;
        (Some(cells), flag)
    });
    let res = vm.alloc_ordinary();
    match next {
        Some(cells) => {
            let value = row_to_js(vm, read_big_ints, &cells)?;
            set_module_prop(vm, res, "done", Value::Boolean(false))?;
            set_module_prop(vm, res, "value", Value::Object(value))?;
        }
        None => {
            set_module_prop(vm, res, "done", Value::Boolean(true))?;
        }
    }
    Ok(Value::Object(res))
}

/// `stmt.setReadBigInts(bool)`：切换 INTEGER 列读取为 bigint。
fn stmt_set_read_big_ints(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let flag = args.first().copied().unwrap_or(Value::Undefined);
    let flag = vm.truthy(flag);
    with_map(&STMTS, |m| {
        if let Some(entry) = m.get_mut(&r.0) {
            entry.read_big_ints = flag;
        }
    });
    Ok(Value::Undefined)
}

/// `stmt.columns()`：列信息数组（`{ name, type: "" }`）。
fn stmt_columns(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let key = current_stmt_key(vm)?;
    let found = stmt_entry(&key).map(|(db_id, sql, _)| (db_id, sql));
    let Some((db_id, sql)) = found else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    let names = with_map(&DBS, |m| {
        let Some(entry) = m.get_mut(&db_id) else {
            return Vec::new();
        };
        match entry.conn.prepare(&sql) {
            Ok(stmt) => stmt
                .column_names()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    });
    let mut elems: Vec<Value> = Vec::with_capacity(names.len());
    for name in names {
        let ci = vm.alloc_ordinary();
        let name_v = Value::Object(vm.alloc_string(name));
        set_module_prop(vm, ci, "name", name_v)?;
        let empty = Value::Object(vm.alloc_string(String::new()));
        set_module_prop(vm, ci, "type", empty)?;
        elems.push(Value::Object(ci));
    }
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// 当前语句实例句柄索引（语句处理器入口共用；非对象接收者属内部不变量破坏）。
fn current_stmt_key(vm: &mut Vm) -> Result<u32, VmError> {
    let receiver = current_receiver();
    match receiver {
        Value::Object(r) => Ok(r.0),
        _ => Err(make_error(
            vm,
            "Error",
            "node:sqlite: receiver is not a StatementSync",
        )),
    }
}

/// 读取语句条目 `(db_id, sql, read_big_ints)`。
fn stmt_entry(key: &u32) -> Option<(u32, String, bool)> {
    with_map(&STMTS, |m| {
        m.get(key)
            .map(|e| (e.db_id, e.sql.clone(), e.read_big_ints))
    })
}

/// 读取语句的 bigint 开关（未登记时 `None`）。
fn stmt_read_big_ints(key: &u32) -> Option<bool> {
    stmt_entry(key).map(|(_, _, flag)| flag)
}

/// 语句执行公共骨架：登记的 SQL → 连接重编译 → 绑定执行闭包。
///
/// 返回 `Err(String)` 表示驱动层错误文本（由调用方包装为 `node:sqlite:` 异常）。
fn exec_on_stmt<T, F>(key: &u32, f: F, plan: &BindPlan) -> Result<T, String>
where
    F: FnOnce(&mut Connection, &str, &BindPlan) -> Result<T, String>,
{
    let (db_id, sql) = stmt_entry(key)
        .map(|(db_id, sql, _)| (db_id, sql))
        .ok_or_else(|| "node:sqlite: sql: database is closed".to_owned())?;
    with_map(&DBS, |m| {
        let Some(entry) = m.get_mut(&db_id) else {
            return Err("node:sqlite: sql: database is closed".to_owned());
        };
        f(&mut entry.conn, &sql, plan)
    })
}

/// 把 `exec_on_stmt` 的 `Err(String)` 转为 `node:sqlite: …` 异常。
fn map_stmt_result<T>(vm: &mut Vm, outcome: Result<T, String>) -> Result<T, VmError> {
    match outcome {
        Ok(v) => Ok(v),
        Err(msg) => {
            // 驱动层文本统一补前缀；内部已带 `node:sqlite:` 的原样透传。
            let full = if msg.starts_with("node:sqlite:") {
                msg
            } else {
                format!("node:sqlite: {msg}")
            };
            Err(sqlite_throw(vm, &full))
        }
    }
}

/// 把绑定计划绑到语句上：位置参数校验数量，命名参数校验占位名。
fn bind_plan(stmt: &mut Statement<'_>, plan: &BindPlan) -> Result<(), String> {
    let param_count = stmt.parameter_count();
    match plan {
        BindPlan::Positional(params) => {
            if params.len() < param_count {
                // 对齐 modernc 驱动：缺参按首个缺失下标报错（1 起）。
                return Err(format!(
                    "node:sqlite: missing argument with index {}",
                    params.len() + 1
                ));
            }
            for (i, p) in params.iter().take(param_count).enumerate() {
                stmt.raw_bind_parameter(i + 1, p.to_sql_value())
                    .map_err(fmt_driver_err_owned)?;
            }
            Ok(())
        }
        BindPlan::Named(pairs) => {
            let provided: HashMap<String, SqlParam> = pairs.iter().cloned().collect();
            for i in 1..=param_count {
                let Some(pname) = stmt.parameter_name(i) else {
                    // "?" 占位遇上纯命名参数：对齐 modernc 按序号报缺参。
                    return Err(format!("node:sqlite: missing argument with index {i}"));
                };
                let key = pname
                    .trim_start_matches(':')
                    .trim_start_matches('@')
                    .trim_start_matches('$');
                let Some(value) = provided.get(key) else {
                    return Err(format!("node:sqlite: missing named argument \"{key}\""));
                };
                stmt.raw_bind_parameter(i, value.to_sql_value())
                    .map_err(fmt_driver_err_owned)?;
            }
            Ok(())
        }
    }
}

/// JS 参数列表 → 绑定计划：单对象参数（无 `length` 属性且非空）视为命名参数。
fn to_bind_plan(vm: &mut Vm, args: &[Value]) -> Result<BindPlan, VmError> {
    if args.len() == 1 {
        if let Value::Object(r) = args[0] {
            let length_undefined = matches!(
                vm.get_property(args[0], "length"),
                Ok(Value::Undefined) | Err(_)
            );
            if length_undefined {
                let pairs: Vec<(String, Value)> = match vm.heap.get(r.index()) {
                    Some(HeapObject::Ordinary { .. }) => vm.own_entries(r.index()),
                    _ => Vec::new(),
                };
                if !pairs.is_empty() {
                    let mut named = Vec::with_capacity(pairs.len());
                    for (k, v) in pairs {
                        named.push((k.clone(), js_to_param(vm, v)?));
                    }
                    return Ok(BindPlan::Named(named));
                }
            }
        }
    }
    let mut positional = Vec::with_capacity(args.len());
    for a in args {
        positional.push(js_to_param(vm, *a)?);
    }
    Ok(BindPlan::Positional(positional))
}

/// JS 值 → 驱动参数（布尔不可绑定，对齐 Node 22 实测）。
fn js_to_param(vm: &mut Vm, v: Value) -> Result<SqlParam, VmError> {
    match v {
        Value::Undefined | Value::Null => Ok(SqlParam::Null),
        Value::Boolean(_) => Err(type_error_throw(
            vm,
            "node:sqlite: provided value cannot be bound to SQLite parameter",
        )),
        Value::Number(n) => {
            if n.is_finite() && n == n.trunc() && n.abs() <= 9.2e18 {
                Ok(SqlParam::Int(n as i64))
            } else {
                Ok(SqlParam::Real(n))
            }
        }
        Value::Object(r) => {
            // 字符串优先按文本绑定（先于 Buffer 字节提取）。
            if let Some(HeapObject::String(s)) = vm.heap.get(r.index()) {
                return Ok(SqlParam::Text(s.clone()));
            }
            if let Some(HeapObject::BigInt(digits)) = vm.heap.get(r.index()) {
                if let Ok(i) = digits.parse::<i64>() {
                    return Ok(SqlParam::Int(i));
                }
                // 超出 int64 的 bigint：按文本存储（对齐 Go 近似策略）。
                return Ok(SqlParam::Text(digits.clone()));
            }
            if let Some(bytes) = crate::builtins::buffer::extract_bytes(vm, v) {
                return Ok(SqlParam::Blob(bytes));
            }
            // 普通对象参数按文本绑定（对齐 Go `v.String()` 落点）。
            Ok(SqlParam::Text(vm.format_value(v)))
        }
    }
}

/// 一行扫描结果 → JS 行对象（属性按列序写入，重名列后者覆盖）。
fn row_to_js(
    vm: &mut Vm,
    read_big_ints: bool,
    cells: &[(String, SqlValue)],
) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for (name, value) in cells {
        let v = sql_value_to_js(vm, read_big_ints, value);
        set_module_prop(vm, obj, name, v)?;
    }
    Ok(obj)
}

/// `ValueRef` → 所有权 `SqlValue`（脱离行借用）。
fn owned_value(v: ValueRef<'_>) -> SqlValue {
    match v {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(i) => SqlValue::Integer(i),
        ValueRef::Real(f) => SqlValue::Real(f),
        ValueRef::Text(b) => SqlValue::Text(String::from_utf8_lossy(b).into_owned()),
        ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
    }
}

/// 驱动值 → JS 值（null/number/bigint/string/Buffer）。
fn sql_value_to_js(vm: &mut Vm, read_big_ints: bool, value: &SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => {
            if read_big_ints {
                Value::Object(vm.alloc_bigint(i.to_string()))
            } else {
                Value::Number(*i as f64)
            }
        }
        SqlValue::Real(f) => Value::Number(*f),
        SqlValue::Text(s) => Value::Object(vm.alloc_string(s.clone())),
        SqlValue::Blob(b) => Value::Object(crate::builtins::buffer::create_buffer_instance(
            vm,
            b.clone(),
        )),
    }
}

/// `fmt_driver_err` 的值参形态（适配 `Result::map_err`）。
fn fmt_driver_err_owned(e: rusqlite::Error) -> String {
    fmt_driver_err(&e)
}

/// rusqlite 错误 → modernc 风格文本（`errstr: errmsg (ext)` / `errstr (ext)`）。
fn fmt_driver_err(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => fmt_code_msg(f.extended_code, msg.as_deref()),
        rusqlite::Error::SqlInputError { error, msg, .. } => {
            fmt_code_msg(error.extended_code, Some(msg))
        }
        other => format!("{other}"),
    }
}

/// 按 modernc 驱动的 `errstr(rc)` 语义拼装：errmsg 与 errstr 同文时省略前缀。
fn fmt_code_msg(ext: i32, msg: Option<&str>) -> String {
    let estr = sqlite_errstr((ext & 0xFF) as u8);
    match msg {
        Some(m) if m == estr => format!("{estr} ({ext})"),
        Some(m) => format!("{estr}: {m} ({ext})"),
        None => format!("{estr} ({ext})"),
    }
}

/// `Connection::open` 失败文本：libsqlite3-sys 对 CANTOPEN 会把路径缀在
/// errmsg 后，modernc 没有；对齐 Go 观测统一取 errstr 文本。
fn fmt_open_err(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, _) => {
            let ext = f.extended_code;
            format!("{} ({ext})", sqlite_errstr((ext & 0xFF) as u8))
        }
        other => fmt_driver_err(other),
    }
}

/// `sqlite3_errstr` 主码文本表（与 C 实现一致的高频子集）。
fn sqlite_errstr(primary: u8) -> &'static str {
    match primary {
        0 => "not an error",
        1 => "SQL logic error",
        2 => "internal logic error",
        3 => "access permission denied",
        4 => "query aborted",
        5 => "database is locked",
        6 => "database table is locked",
        7 => "out of memory",
        8 => "attempt to write a readonly database",
        9 => "interrupted",
        10 => "disk I/O error",
        11 => "database disk image is malformed",
        12 => "unknown operation",
        13 => "database or disk is full",
        14 => "unable to open database file",
        15 => "locking protocol",
        16 => "database is empty",
        17 => "database schema has changed",
        18 => "string or blob too big",
        19 => "constraint failed",
        20 => "datatype mismatch",
        21 => "bad parameter or other API misuse",
        22 => "large file support is disabled",
        23 => "authorization denied",
        25 => "column index out of range",
        26 => "file is not a database",
        27 => "notification message",
        28 => "warning message",
        _ => "unknown error",
    }
}

/// 命名空间属性值（堆字符串，供 `try_dispatch` 通用实例分派读取）。
fn ns_value(vm: &mut Vm, ns: &str) -> Value {
    Value::Object(vm.alloc_string(ns.to_owned()))
}

/// 抛 Node 错误对象（name=Error）。
fn sqlite_throw(vm: &mut Vm, msg: &str) -> VmError {
    make_error(vm, "Error", msg)
}

/// 抛 TypeError 错误对象。
fn type_error_throw(vm: &mut Vm, msg: &str) -> VmError {
    make_error(vm, "TypeError", msg)
}

/// 构造带 `name`/`message` 属性的错误实例并包装为 VM 异常。
fn make_error(vm: &mut Vm, name: &str, msg: &str) -> VmError {
    let obj = vm.alloc_ordinary();
    let name_v = Value::Object(vm.alloc_string(name.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "name", name_v);
    let msg_v = Value::Object(vm.alloc_string(msg.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "message", msg_v);
    VmError::Thrown(Value::Object(obj))
}

/// 判断值是否可调用（Closure / NativeFn / NativeCtor）。
fn is_callable_value(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. } | HeapObject::NativeCtor { .. })
    ))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = database_sync_ctor;
        let _: crate::builtins::BuiltinHandler = db_exec;
        let _: crate::builtins::BuiltinHandler = db_prepare;
        let _: crate::builtins::BuiltinHandler = db_close;
        let _: crate::builtins::BuiltinHandler = db_transaction;
        let _: crate::builtins::BuiltinHandler = txn_call;
        let _: crate::builtins::BuiltinHandler = stmt_run;
        let _: crate::builtins::BuiltinHandler = stmt_get;
        let _: crate::builtins::BuiltinHandler = stmt_all;
        let _: crate::builtins::BuiltinHandler = stmt_iterate;
        let _: crate::builtins::BuiltinHandler = iter_next;
        let _: crate::builtins::BuiltinHandler = stmt_set_read_big_ints;
        let _: crate::builtins::BuiltinHandler = stmt_columns;
    }

    #[test]
    fn errstr_table_anchor() {
        assert_eq!(sqlite_errstr(1), "SQL logic error");
        assert_eq!(sqlite_errstr(14), "unable to open database file");
        assert_eq!(sqlite_errstr(19), "constraint failed");
    }
}
