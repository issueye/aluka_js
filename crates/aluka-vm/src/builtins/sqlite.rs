//! `sqlite` 内置模块（Phase 7）：Node 22 `node:sqlite` 原生 `DatabaseSync`。
//!
//! 语义严格对齐 Node.js 22 LTS（v22.23.1 实测）：
//! - `new DatabaseSync(path)`（别名 `Database`）打开数据库（`:memory:` 或文件路径）；
//! - `db.exec(sql)` 执行多语句；`db.prepare(sql) -> StatementSync`；
//!   `db.close()`；`isOpen`/`isTransaction` 数据属性；
//! - `StatementSync.run/get/all/iterate/setReadBigInts/columns/sourceSQL`，
//!   位置与命名参数；绑定规则（缺位 NULL 补齐/超位 column index out of
//!   range/未知命名键 Unknown named parameter/undefined 与布尔 TypeError）；
//! - 值类型：`null` / `number` / `bigint`（`setReadBigInts`）/ `string` /
//!   Uint8Array（Buffer 亦按 blob）；BLOB 读回纯 `Uint8Array`；
//! - 错误对象对齐 Node 22 实测：`message` = errmsg 原文（无 `node:sqlite:`
//!   前缀与扩展码尾缀），SQL 错误挂 `code: ERR_SQLITE_ERROR` +
//!   `errcode`（扩展码，如 UNIQUE 撞 INTEGER PRIMARY KEY = 1555）+ `errstr`
//!   （主码文本）；参数校验 TypeError 挂 `code: ERR_INVALID_ARG_TYPE`；
//! - 事务控制以 `exec(BEGIN/COMMIT/ROLLBACK)` + `isTransaction` 对齐 Node 22；
//!   **`db.transaction(fn)` 为 Aluka 超集扩展**（Node 22 LTS 无此方法，Node
//!   23.8+ 才有；保留以便 better-sqlite3 风格代码迁移，不进 Node 对拍）。
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
use std::cell::RefCell;
use std::collections::HashMap;

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

// 线程局部状态表（键为实例堆句柄索引；堆句柄仅本线程 Vm 有效）。
thread_local! {
    static DBS: RefCell<Option<HashMap<u32, DbEntry>>> = const { RefCell::new(None) };
    static STMTS: RefCell<Option<HashMap<u32, StmtEntry>>> = const { RefCell::new(None) };
    static ITERS: RefCell<Option<HashMap<u32, IterEntry>>> = const { RefCell::new(None) };
    static TXNS: RefCell<Option<HashMap<u32, TxEntry>>> = const { RefCell::new(None) };
}

/// 在状态表上执行闭包（惰性初始化；各表独立借用、不嵌套，避免借用冲突）。
fn with_map<T, F, R>(m: &RefCell<Option<HashMap<u32, T>>>, f: F) -> R
where
    F: FnOnce(&mut HashMap<u32, T>) -> R,
{
    let mut guard = m.borrow_mut();
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

/// 取 JS 字符串堆值(非字符串返回 `None`)。
fn as_string_value(vm: &Vm, v: Value) -> Option<String> {
    match v {
        Value::Object(r) => match vm.heap.get(r.index()) {
            Some(HeapObject::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `new DatabaseSync(path)`：打开数据库连接并返回实例。
fn database_sync_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let raw = args.first().copied().unwrap_or(Value::Undefined);
    // Node 22 validator：path 必须为 string / Uint8Array / URL，否则 TypeError。
    let path = match as_string_value(vm, raw) {
        Some(s) => s,
        None => {
            // Buffer/Uint8Array 字节按 UTF-8 无损转换（Node 接受 Uint8Array 路径）。
            if let Value::Object(_) = raw {
                if let Some(bytes) = crate::builtins::buffer::extract_bytes(vm, raw) {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    return Err(type_error_throw(
                        vm,
                        "The \"path\" argument must be a string, Uint8Array, or URL without null bytes.",
                    ));
                }
            } else {
                return Err(type_error_throw(
                    vm,
                    "The \"path\" argument must be a string, Uint8Array, or URL without null bytes.",
                ));
            }
        }
    };
    let conn = Connection::open(&path).map_err(|e| sqlite_throw_ext(vm, SqErr::from_driver(&e)))?;

    let obj = vm.alloc_ordinary();
    let ns = ns_value(vm, "sqlite:db");
    set_module_prop(vm, obj, "_builtinNs", ns)?;
    for method in ["exec", "prepare", "close", "transaction"] {
        let fn_ref = vm.alloc_native_fn(&format!("sqlite:db.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    // `isOpen` 以数据属性维护（open 时 true，close 后置 false）。
    set_module_prop(vm, obj, "isOpen", Value::Boolean(true))?;
    set_module_prop(vm, obj, "isTransaction", Value::Boolean(false))?;

    let id = obj.0;
    DBS.with(|g| {
        with_map(g, |m| {
            m.insert(id, DbEntry { conn });
        })
    });
    Ok(Value::Object(obj))
}

/// `db.exec(sql)`：执行一段（可含多语句的）SQL，无返回行。
fn db_exec(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let sql = args
        .first()
        .copied()
        .and_then(|v| as_string_value(vm, v))
        .ok_or_else(|| type_error_throw(vm, "The \"sql\" argument must be a string."))?;
    let id = require_db_id(vm)?;
    let outcome = DBS.with(|g| {
        with_map(g, |m| {
            let Some(entry) = m.get_mut(&id) else {
                return Err(SqErr::text("database is not open"));
            };
            entry
                .conn
                .execute_batch(&sql)
                .map_err(|e| SqErr::from_driver(&e))
        })
    });
    match outcome {
        Ok(()) => {
            sync_is_transaction(vm, id, &sql);
            Ok(Value::Undefined)
        }
        Err(sq) => Err(sqlite_throw_ext(vm, sq)),
    }
}

/// 依 exec 的 SQL 首 token 维护 `isTransaction` 数据属性
/// （BEGIN → true；COMMIT/ROLLBACK/END → false；其余不动）。
fn sync_is_transaction(vm: &mut Vm, id: u32, sql: &str) {
    let first = sql
        .split(|c: char| c.is_whitespace() || c == ';')
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    let in_txn = match first.as_str() {
        "BEGIN" => Some(true),
        "COMMIT" | "ROLLBACK" | "END" => Some(false),
        _ => None,
    };
    if let Some(v) = in_txn {
        // DBS 表键即数据库对象句柄
        let _ = set_module_prop(vm, ObjectRef(id), "isTransaction", Value::Boolean(v));
    }
}

/// `db.prepare(sql)`：编译语句并返回 `StatementSync` 实例。
fn db_prepare(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let sql = args
        .first()
        .copied()
        .and_then(|v| as_string_value(vm, v))
        .ok_or_else(|| type_error_throw(vm, "The \"sql\" argument must be a string."))?;
    let db_id = require_db_id(vm)?;
    // prepare 期即校验语法（对齐 Node：非法 SQL 在 prepare 时报错）。
    let prep = DBS.with(|g| {
        with_map(g, |m| {
            let Some(entry) = m.get_mut(&db_id) else {
                return Err(SqErr::text("database is not open"));
            };
            entry
                .conn
                .prepare(&sql)
                .map(|_| ())
                .map_err(|e| SqErr::from_driver(&e))
        })
    });
    if let Err(sq) = prep {
        return Err(sqlite_throw_ext(vm, sq));
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
    STMTS.with(|g| {
        with_map(g, |m| {
            m.insert(
                id,
                StmtEntry {
                    db_id,
                    sql,
                    read_big_ints: false,
                },
            );
        })
    });
    Ok(Value::Object(obj))
}

/// `db.close()`：关闭连接、清理语句并把 `isOpen` 置 false（二次 close 报错）。
fn db_close(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let id = r.0;
    STMTS.with(|g| {
        with_map(g, |m| {
            m.retain(|_, e| e.db_id != id);
        })
    });
    let removed = DBS.with(|g| with_map(g, |m| m.remove(&id).is_some()));
    if !removed {
        // Node：已关闭的连接再 close 报错（先于 isOpen 置位检查）。
        return Err(sqlite_throw_ext(vm, SqErr::text("database is not open")));
    }
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
    TXNS.with(|g| {
        with_map(g, |m| {
            m.insert(wid, TxEntry { db_id, callback });
        })
    });
    Ok(Value::Object(wrapper))
}

/// 事务包装函数调用：BEGIN → 回调 → COMMIT / ROLLBACK。
fn txn_call(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Value::Object(r) = receiver else {
        return Ok(Value::Undefined);
    };
    let found = TXNS.with(|g| with_map(g, |m| m.get(&r.0).map(|e| (e.db_id, e.callback))));
    let Some((db_id, callback)) = found else {
        return Ok(Value::Undefined);
    };
    let begun = DBS.with(|g| {
        with_map(g, |m| {
            let Some(entry) = m.get_mut(&db_id) else {
                return Err(SqErr::text("database is not open"));
            };
            entry
                .conn
                .execute_batch("BEGIN")
                .map_err(|e| SqErr::from_driver(&e))
        })
    });
    if let Err(sq) = begun {
        return Err(sqlite_throw_ext(vm, sq));
    }
    match vm.invoke_callable(callback, Value::Undefined, args) {
        Ok(ret) => {
            let committed = DBS.with(|g| {
                with_map(g, |m| {
                    let Some(entry) = m.get_mut(&db_id) else {
                        return Err(SqErr::text("database is not open"));
                    };
                    entry
                        .conn
                        .execute_batch("COMMIT")
                        .map_err(|e| SqErr::from_driver(&e))
                })
            });
            match committed {
                Ok(()) => Ok(ret),
                Err(sq) => {
                    rollback_quiet(db_id);
                    Err(sqlite_throw_ext(vm, sq))
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
    DBS.with(|g| {
        with_map(g, |m| {
            if let Some(entry) = m.get_mut(&db_id) {
                let _ = entry.conn.execute_batch("ROLLBACK");
            }
        })
    });
}

/// 取当前接收者（DatabaseSync 实例）的句柄索引。
fn require_db_id(vm: &mut Vm) -> Result<u32, VmError> {
    let receiver = current_receiver();
    match receiver {
        Value::Object(r) => Ok(r.0),
        _ => Err(sqlite_throw(
            vm,
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
///
/// 对齐 Node 22：对返回行的语句（SELECT）`run` 不报错——changes /
/// lastInsertRowid 回读最近一次写操作的值（sqlite3_changes 语义）。
fn run_inner(conn: &mut Connection, sql: &str, plan: &BindPlan) -> Result<(i64, i64), SqErr> {
    let changes = {
        let mut stmt = conn.prepare(sql).map_err(|e| SqErr::from_driver(&e))?;
        bind_plan(&mut stmt, plan)?;
        match stmt.raw_execute() {
            Ok(n) => n as i64,
            Err(rusqlite::Error::ExecuteReturnedResults) => conn.changes() as i64,
            Err(e) => return Err(SqErr::from_driver(&e)),
        }
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
) -> Result<Option<Vec<(String, SqlValue)>>, SqErr> {
    let mut stmt = conn.prepare(sql).map_err(|e| SqErr::from_driver(&e))?;
    bind_plan(&mut stmt, plan)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut rows = stmt.raw_query();
    match rows.next().map_err(|e| SqErr::from_driver(&e))? {
        Some(row) => {
            let mut cells = Vec::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let v = row.get_ref(i).map_err(|e| SqErr::from_driver(&e))?;
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
    ITERS.with(|g| {
        with_map(g, |m| {
            m.insert(
                id,
                IterEntry {
                    rows,
                    pos: 0,
                    read_big_ints,
                },
            );
        })
    });
    Ok(Value::Object(iter))
}

/// `all`/`iterate` 共用内核：执行查询并物化全部行。
fn rows_inner(
    conn: &mut Connection,
    sql: &str,
    plan: &BindPlan,
) -> Result<Vec<Vec<(String, SqlValue)>>, SqErr> {
    let mut stmt = conn.prepare(sql).map_err(|e| SqErr::from_driver(&e))?;
    bind_plan(&mut stmt, plan)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut rows = stmt.raw_query();
    let mut out: Vec<Vec<(String, SqlValue)>> = Vec::new();
    loop {
        match rows.next().map_err(|e| SqErr::from_driver(&e))? {
            Some(row) => {
                let mut cells = Vec::with_capacity(names.len());
                for (i, name) in names.iter().enumerate() {
                    let v = row.get_ref(i).map_err(|e| SqErr::from_driver(&e))?;
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
    let (next, read_big_ints) = ITERS.with(|g| {
        with_map(g, |m| {
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
        })
    });
    let res = vm.alloc_ordinary();
    match next {
        Some(cells) => {
            let value = row_to_js(vm, read_big_ints, &cells)?;
            set_module_prop(vm, res, "done", Value::Boolean(false))?;
            set_module_prop(vm, res, "value", Value::Object(value))?;
        }
        None => {
            // Node 22：迭代结束后 next() 仍返回带 value: null 的 done 结果。
            set_module_prop(vm, res, "done", Value::Boolean(true))?;
            set_module_prop(vm, res, "value", Value::Null)?;
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
    STMTS.with(|g| {
        with_map(g, |m| {
            if let Some(entry) = m.get_mut(&r.0) {
                entry.read_big_ints = flag;
            }
        })
    });
    Ok(Value::Undefined)
}

/// `stmt.columns()`：列信息数组（Node 22 五键：column/database/name/table/type；
/// 表达式列置 null；type 取声明类型 decltype）。源信息经 rusqlite 安全 API
/// `columns()` / `columns_with_metadata()`（origin/table/database 与 Node
/// sqlite3_column_*_name 同源，alias 投影也能取到源列名）。
fn stmt_columns(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let key = current_stmt_key(vm)?;
    let found = stmt_entry(&key).map(|(db_id, sql, _)| (db_id, sql));
    let Some((db_id, sql)) = found else {
        return Ok(Value::Object(vm.alloc_array(Vec::new())));
    };
    // 每列 (origin, database, name, table, decltype)。
    type ColMeta = (
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    );
    let cols: Vec<ColMeta> = DBS.with(|g| {
        with_map(g, |m| {
            let Some(entry) = m.get_mut(&db_id) else {
                return Vec::new();
            };
            match entry.conn.prepare(&sql) {
                Ok(stmt) => {
                    let metas = stmt.columns_with_metadata();
                    let decls = stmt.columns();
                    let mut out = Vec::with_capacity(metas.len());
                    for (i, meta) in metas.iter().enumerate() {
                        let decltype = decls.get(i).and_then(|c| c.decl_type());
                        out.push((
                            meta.origin_name().map(str::to_owned),
                            meta.database_name().map(str::to_owned),
                            meta.name().to_owned(),
                            meta.table_name().map(str::to_owned),
                            decltype.map(str::to_owned),
                        ));
                    }
                    out
                }
                Err(_) => Vec::new(),
            }
        })
    });
    let mut elems: Vec<Value> = Vec::with_capacity(cols.len());
    for (origin, database, name, table, decltype) in cols {
        let ci = vm.alloc_ordinary();
        // 键序对齐 Node：column → database → name → table → type。
        // （先取字符串句柄再写属性，避免同一表达式内双重可变借用。）
        let o = origin.map(|s| vm.alloc_string(s));
        set_module_prop(
            vm,
            ci,
            "column",
            o.map(Value::Object).unwrap_or(Value::Null),
        )?;
        let d = database.map(|s| vm.alloc_string(s));
        set_module_prop(
            vm,
            ci,
            "database",
            d.map(Value::Object).unwrap_or(Value::Null),
        )?;
        let n = vm.alloc_string(name);
        set_module_prop(vm, ci, "name", Value::Object(n))?;
        let t = table.map(|s| vm.alloc_string(s));
        set_module_prop(vm, ci, "table", t.map(Value::Object).unwrap_or(Value::Null))?;
        let ty = decltype.map(|s| vm.alloc_string(s));
        set_module_prop(vm, ci, "type", ty.map(Value::Object).unwrap_or(Value::Null))?;
        elems.push(Value::Object(ci));
    }
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// 当前语句实例句柄索引（语句处理器入口共用；非对象接收者属内部不变量破坏）。
fn current_stmt_key(vm: &mut Vm) -> Result<u32, VmError> {
    let receiver = current_receiver();
    match receiver {
        Value::Object(r) => Ok(r.0),
        _ => Err(sqlite_throw(
            vm,
            "node:sqlite: receiver is not a StatementSync",
        )),
    }
}

/// 读取语句条目 `(db_id, sql, read_big_ints)`。
fn stmt_entry(key: &u32) -> Option<(u32, String, bool)> {
    STMTS.with(|g| {
        with_map(g, |m| {
            m.get(key)
                .map(|e| (e.db_id, e.sql.clone(), e.read_big_ints))
        })
    })
}

/// 读取语句的 bigint 开关（未登记时 `None`）。
fn stmt_read_big_ints(key: &u32) -> Option<bool> {
    stmt_entry(key).map(|(_, _, flag)| flag)
}

/// 语句执行公共骨架：登记的 SQL → 连接重编译 → 绑定执行闭包。
///
/// 返回 `Err(SqErr)` 表示驱动层错误（消息 + 扩展码，由调用方抛 Node 形态异常）。
fn exec_on_stmt<T, F>(key: &u32, f: F, plan: &BindPlan) -> Result<T, SqErr>
where
    F: FnOnce(&mut Connection, &str, &BindPlan) -> Result<T, SqErr>,
{
    let (db_id, sql) = stmt_entry(key)
        .map(|(db_id, sql, _)| (db_id, sql))
        .ok_or_else(|| SqErr::text("statement has been finalized"))?;
    DBS.with(|g| {
        with_map(g, |m| {
            let Some(entry) = m.get_mut(&db_id) else {
                return Err(SqErr::text("database is not open"));
            };
            f(&mut entry.conn, &sql, plan)
        })
    })
}

/// 把 `exec_on_stmt` 的 `Err(SqErr)` 转为 Node 22 形态异常。
fn map_stmt_result<T>(vm: &mut Vm, outcome: Result<T, SqErr>) -> Result<T, VmError> {
    match outcome {
        Ok(v) => Ok(v),
        Err(sq) => Err(sqlite_throw_ext(vm, sq)),
    }
}

/// 把绑定计划绑到语句上（对齐 Node 22.23.1 实测语义）：
/// - 位置参数：缺位保持未绑定（SQLite 视 NULL，不报错）；**超位**报
///   `column index out of range`；
/// - 命名参数：先校验传入键均存在于语句命名占位符，未知键报
///   `Unknown named parameter 'k'`；语句占位符无对应值（含 `?` 无名占位）
///   保持未绑定 → NULL。
fn bind_plan(stmt: &mut Statement<'_>, plan: &BindPlan) -> Result<(), SqErr> {
    let param_count = stmt.parameter_count();
    match plan {
        BindPlan::Positional(params) => {
            if params.len() > param_count {
                // Node 实测：多余位置参数报列越界文本（errcode SQLITE_RANGE=25）。
                return Err(SqErr::text("column index out of range"));
            }
            for (i, p) in params.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, p.to_sql_value())
                    .map_err(|e| SqErr::from_driver(&e))?;
            }
            Ok(())
        }
        BindPlan::Named(pairs) => {
            // 语句命名占位符名集合（去 `:` / `@` / `$` 前缀）。
            let mut names: Vec<String> = Vec::new();
            for i in 1..=param_count {
                if let Some(pname) = stmt.parameter_name(i) {
                    names.push(
                        pname
                            .trim_start_matches(':')
                            .trim_start_matches('@')
                            .trim_start_matches('$')
                            .to_owned(),
                    );
                }
            }
            // 未知命名键 → Node "Unknown named parameter 'y'"。
            for (k, _) in pairs {
                if !names.iter().any(|n| n == k) {
                    return Err(SqErr::text(format!("Unknown named parameter '{k}'")));
                }
            }
            // 逐个命名占位符绑定（缺值保持 NULL；`?` 无名占位不绑）。
            for i in 1..=param_count {
                let Some(pname) = stmt.parameter_name(i) else {
                    continue;
                };
                let key = pname
                    .trim_start_matches(':')
                    .trim_start_matches('@')
                    .trim_start_matches('$');
                if let Some((_, value)) = pairs.iter().find(|(k, _)| k == key) {
                    stmt.raw_bind_parameter(i, value.to_sql_value())
                        .map_err(|e| SqErr::from_driver(&e))?;
                }
            }
            Ok(())
        }
    }
}

/// JS 参数列表 → 绑定计划（对齐 Node 22.23.1）：
/// - 单对象参数且对象**不是**可直接绑定的字符串/BigInt/blob → 按命名参数展开
///   （普通对象键、数组数字键一致展开；空对象 → 空命名集，占位保持 NULL）；
/// - 其余一律位置参数（各值按位绑定，不可绑定值报带序号的 TypeError）。
fn to_bind_plan(vm: &mut Vm, args: &[Value]) -> Result<BindPlan, VmError> {
    if args.len() == 1 {
        if let Value::Object(r) = args[0] {
            // 可直绑对象：字符串/BigInt/blob 载体（Buffer/TypedArray/ArrayBuffer/
            // DataView）。**Array 除外**——Node 把数组参数按命名参数展开
            // （数字键），extract_bytes 对数组会误判为字节序列。
            let is_array = matches!(vm.heap.get(r.index()), Some(HeapObject::Array { .. }));
            let bindable = !is_array
                && (matches!(
                    vm.heap.get(r.index()),
                    Some(HeapObject::String(_) | HeapObject::BigInt(_))
                ) || crate::builtins::buffer::extract_bytes(vm, args[0]).is_some());
            if !bindable {
                let pairs: Vec<(String, Value)> = match vm.heap.get(r.index()) {
                    Some(HeapObject::Ordinary { .. }) => vm.own_entries(r.index()),
                    Some(HeapObject::Array { elements, .. }) => elements
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (i.to_string(), *v))
                        .collect(),
                    _ => Vec::new(),
                };
                let mut named = Vec::with_capacity(pairs.len());
                for (k, v) in pairs {
                    named.push((k.clone(), js_to_param(vm, v, 1)?));
                }
                return Ok(BindPlan::Named(named));
            }
        }
    }
    let mut positional = Vec::with_capacity(args.len());
    for (i, a) in args.iter().enumerate() {
        positional.push(js_to_param(vm, *a, i + 1)?);
    }
    Ok(BindPlan::Positional(positional))
}

/// 不可绑定值的 TypeError（Node 文本带 1 起参数序号）。
fn not_bindable(vm: &mut Vm, param_idx: usize) -> VmError {
    type_error_throw(
        vm,
        &format!("Provided value cannot be bound to SQLite parameter {param_idx}."),
    )
}

/// JS 值 → 驱动参数（对齐 Node 22 实测：`null` → NULL；`undefined`/布尔/
/// 普通对象/数组 → TypeError；number 整值入 INTEGER；bigint 超 i64 按文本近似）。
fn js_to_param(vm: &mut Vm, v: Value, param_idx: usize) -> Result<SqlParam, VmError> {
    match v {
        Value::Undefined => Err(not_bindable(vm, param_idx)),
        Value::Null => Ok(SqlParam::Null),
        Value::Boolean(_) => Err(not_bindable(vm, param_idx)),
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
                // 超出 int64 的 bigint：按文本存储（近似策略，已登记）。
                return Ok(SqlParam::Text(digits.clone()));
            }
            // 数组不是合法 SQLite 绑定值（Node：多参数位置绑定时报 TypeError；
            // 单数组参数在上层已按命名展开）。必须在 extract_bytes 之前排除——
            // 后者会把数组元素当字节序列。
            if matches!(vm.heap.get(r.index()), Some(HeapObject::Array { .. })) {
                return Err(not_bindable(vm, param_idx));
            }
            if let Some(bytes) = crate::builtins::buffer::extract_bytes(vm, v) {
                return Ok(SqlParam::Blob(bytes));
            }
            // 普通对象/函数等：不可绑定（Node 22 抛 TypeError）。
            Err(not_bindable(vm, param_idx))
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
        // 对齐 Node 22：BLOB 读出为纯 Uint8Array（非 Buffer 子类）
        SqlValue::Blob(b) => {
            let ab = vm.alloc_array_buffer(b.clone(), false, false, 0);
            Value::Object(vm.alloc_typed_array(
                crate::typed_array::TypedKind::Uint8,
                ab,
                0,
                b.len(),
            ))
        }
    }
}

/// 驱动层错误（Node 22 形态）：消息文本 + SQLite 扩展错误码。
#[derive(Debug)]
struct SqErr {
    /// 错误消息（Node `message` = sqlite3_errmsg 原文，无前缀无扩展码尾缀）
    msg: String,
    /// sqlite3_extended_errcode（Node `errcode` 属性；0 = 非 SQLite 驱动错误）
    ext: i32,
}

impl SqErr {
    /// rusqlite 错误 → Node 22 语义（消息 = errmsg，码 = extended）。
    fn from_driver(e: &rusqlite::Error) -> SqErr {
        match e {
            rusqlite::Error::SqliteFailure(f, msg) => {
                let primary = (f.extended_code & 0xFF) as u8;
                // CANTOPEN：rusqlite 的 errmsg 会缀上失败路径
                // （"unable to open database file: Z:/…"），Node 实测只报
                // errstr 原文，统一裁到 errstr 短语。
                let text = if primary == 14 {
                    sqlite_errstr(primary).to_owned()
                } else {
                    msg.clone()
                        .unwrap_or_else(|| sqlite_errstr(primary).to_owned())
                };
                SqErr {
                    msg: text,
                    ext: f.extended_code,
                }
            }
            rusqlite::Error::SqlInputError { error, msg, .. } => SqErr {
                msg: msg.clone(),
                ext: error.extended_code,
            },
            other => SqErr {
                msg: format!("{other}"),
                ext: 0,
            },
        }
    }

    /// 纯文本错误（Node 侧校验错误：column index out of range / Unknown named
    /// parameter / database is not open 等，无 SQLite 驱动码）。
    fn text(msg: impl Into<String>) -> SqErr {
        SqErr {
            msg: msg.into(),
            ext: 0,
        }
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

/// 抛 Node 形态错误对象（name=Error；code=ERR_SQLITE_ERROR；驱动错误带
/// errcode=扩展码 / errstr=主码文本，对齐 Node 22.23.1 实测属性面）。
fn sqlite_throw_ext(vm: &mut Vm, sq: SqErr) -> VmError {
    let ext = (sq.ext != 0).then_some(sq.ext);
    make_error(vm, "Error", "ERR_SQLITE_ERROR", &sq.msg, ext)
}

/// 抛无驱动码的 Error（内部/校验文本；仍挂 ERR_SQLITE_ERROR code）。
fn sqlite_throw(vm: &mut Vm, msg: &str) -> VmError {
    make_error(vm, "Error", "ERR_SQLITE_ERROR", msg, None)
}

/// 抛 TypeError 错误对象（code=ERR_INVALID_ARG_TYPE，Node validator 形态）。
fn type_error_throw(vm: &mut Vm, msg: &str) -> VmError {
    make_error(vm, "TypeError", "ERR_INVALID_ARG_TYPE", msg, None)
}

/// 构造带 `name`/`message`/`code`（及可选 `errcode`/`errstr`）属性的错误实例。
fn make_error(vm: &mut Vm, name: &str, code: &str, msg: &str, ext: Option<i32>) -> VmError {
    let obj = vm.alloc_ordinary();
    let name_v = Value::Object(vm.alloc_string(name.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "name", name_v);
    let msg_v = Value::Object(vm.alloc_string(msg.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "message", msg_v);
    let code_v = Value::Object(vm.alloc_string(code.to_owned()));
    let _ = vm.set_property(Value::Object(obj), "code", code_v);
    if let Some(ext) = ext {
        let _ = vm.set_property(Value::Object(obj), "errcode", Value::Number(ext as f64));
        let estr = sqlite_errstr((ext & 0xFF) as u8);
        let sv = vm.alloc_string(estr.to_owned());
        let _ = vm.set_property(Value::Object(obj), "errstr", Value::Object(sv));
    }
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
