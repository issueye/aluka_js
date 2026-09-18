//! `util` 内置模块（Phase 2）：`format` / `inspect` / `util.types` 类型判断。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodeutil`）：
//! - `format(...)`：无 `%` 时全部参数空格连接；`%s/%d/%j/%o/%O` 占位消费参数
//!   （`%d` 对齐 Go 先取 `Int()` 截断、`%j` 用 String() 简化输出）；`%%` 转义；
//!   剩余参数补空格追加；
//! - `inspect(v)`：复用 Go `String()` 口径——字符串原样、数组 `[ a, b ]`、
//!   普通对象 `{ k: v }`（键序确定性：本实现按键排序，探测对象按键序插入）；
//! - `util.types.isArray/isString/isNumber/isObject`：`isArray`/`isString` 按
//!   堆形态判定、`isNumber` 按值类型、`isObject` 按对象形态（排除原始类型与
//!   函数）。`util.types` 是独立注册表子模块（`CALL_METHOD` 形态二命中），
//!   - 通过 [`TYPES_MODULE`] 共用 build 期创建的同一对象。
//!
//! `os` 非本模块责任，但如 [`crate::builtins::os`] 所述，任一注册表方法首次
//! 执行时会惰性重链 os 单例；本模块处理器同样在入口调用该惯用法
//! （见 [`sync_os_link`] 的载入说明）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("util")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "util",
    build,
};

/// `util.types` 子模块（与主模块共享 build 期创建的 types 对象）。
pub const TYPES_MODULE: ModuleDef = ModuleDef {
    name: "util/types",
    build: build_types,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let format_fn = vm.alloc_native_fn("util.format");
    let inspect_fn = vm.alloc_native_fn("util.inspect");
    let inherits_fn = vm.alloc_native_fn("util.inherits");
    set_module_prop(vm, obj, "format", Value::Object(format_fn))?;
    set_module_prop(vm, obj, "inspect", Value::Object(inspect_fn))?;
    set_module_prop(vm, obj, "inherits", Value::Object(inherits_fn))?;
    let types = vm.alloc_ordinary();
    set_module_prop(vm, obj, "types", Value::Object(types))?;
    // legacy isX 族（Node 已弃用但保留；真实包顶层仍调用）
    let legacy: [(&str, crate::builtins::BuiltinHandler); 14] = [
        ("isArray", legacy_is_array),
        ("isBoolean", legacy_is_boolean),
        ("isNull", legacy_is_null),
        ("isNullOrUndefined", legacy_is_null_or_undefined),
        ("isNumber", legacy_is_number),
        ("isString", legacy_is_string),
        ("isSymbol", legacy_is_symbol),
        ("isUndefined", legacy_is_undefined),
        ("isObject", legacy_is_object),
        ("isFunction", legacy_is_function),
        ("isBuffer", legacy_is_buffer),
        ("isPrimitive", legacy_is_primitive),
        ("isRegExp", legacy_is_regexp),
        ("isDate", legacy_is_date),
    ];
    for (name, f) in legacy {
        let fn_ref = vm.alloc_native_fn(&format!("util.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
        register_handler(registry, "util", name, f);
    }
    for (name, f) in [
        (
            "isDeepStrictEqual",
            is_deep_strict_equal as crate::builtins::BuiltinHandler,
        ),
        ("getSystemErrorName", get_system_error_name),
        ("getSystemErrorMessage", get_system_error_message),
        ("stripVTControlCharacters", strip_vt_control_characters),
        ("promisify", promisify),
        ("callbackify", callbackify),
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("util.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
        register_handler(registry, "util", name, f);
    }
    register_handler(registry, "util", "format", format);
    register_handler(registry, "util", "inspect", inspect);
    register_handler(registry, "util", "inherits", inherits);
    Ok(obj)
}

/// `util.inherits(ctor, superCtor)`：`ctor.prototype` 的 [[Prototype]] 指向
/// `superCtor.prototype` 且 `constructor` 回指 `ctor`（Node 继承语义；
/// express 的 Router/Route 等原型链依赖）。
fn inherits(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ctor = args.first().copied().unwrap_or(Value::Undefined);
    let super_ctor = args.get(1).copied().unwrap_or(Value::Undefined);
    let proto = vm.get_property(ctor, "prototype")?;
    let super_proto = vm.get_property(super_ctor, "prototype")?;
    if proto.as_object().is_some() {
        let sp = match super_proto.case() {
            ValueCase::Object(r) => Some(r),
            _ => None,
        };
        vm.set_prototype_of(proto, sp);
        let _ = vm.set_property(proto, "constructor", ctor);
    }
    Ok(ctor)
}

/// `util.types` 子模块 build：取主模块 `types` 属性对象并登记类型判断方法。
fn build_types(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let util_mod = registry.module("util").ok_or_else(|| {
        let msg = vm.alloc_string("util.types: 主模块未注册".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    let types_val = vm.get_property(Value::Object(util_mod), "types")?;
    let ValueCase::Object(types) = types_val.case() else {
        let msg = vm.alloc_string("util.types: types 属性缺失".to_owned());
        return Err(VmError::Thrown(Value::Object(msg)));
    };
    register_handler(registry, "util/types", "isArray", is_array);
    register_handler(registry, "util/types", "isString", is_string);
    register_handler(registry, "util/types", "isNumber", is_number);
    register_handler(registry, "util/types", "isObject", is_object);
    // Node 22 的类型谓词面（真实包按这些做能力分支）
    for (name, f) in [
        (
            "isPromise",
            types_is_promise as crate::builtins::BuiltinHandler,
        ),
        ("isDate", types_is_date),
        ("isRegExp", types_is_regexp),
        ("isError", types_is_error),
        ("isMap", types_is_map),
        ("isSet", types_is_set),
        ("isTypedArray", types_is_typed_array),
        ("isAsyncFunction", types_is_async_function),
        ("isPrimitive", types_is_primitive),
        ("isNativeError", types_is_error),
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("util/types.{name}"));
        set_module_prop(vm, types, name, Value::Object(fn_ref))?;
        register_handler(registry, "util/types", name, f);
    }
    Ok(types)
}

/// 惰性重链 os 单例（见 `crate::builtins::os` 模块文档）；本仓所有注册表
/// 处理器入口调用，保证任意探测脚本在首个注册表方法处完成 os 重链。
fn sync_os_link(vm: &mut Vm) {
    if let Some(cur) = vm.os_module {
        if vm.builtin_registry.module("os") != Some(cur) {
            vm.builtin_registry.modules.insert("os", cur);
        }
    }
}

/// `util.format(...)`：Node 风格占位符替换（对齐 Go `utilFormat`）。
fn format(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let fmt = vm.format_value(args[0]);
    if !fmt.contains('%') {
        let parts: Vec<String> = args.iter().map(|a| inspect_value(vm, *a)).collect();
        return Ok(Value::Object(vm.alloc_string(parts.join(" "))));
    }
    let mut out = String::new();
    let mut arg_idx = 1usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                Some(&'s') => {
                    chars.next();
                    if arg_idx < args.len() {
                        out.push_str(&inspect_value(vm, args[arg_idx]));
                        arg_idx += 1;
                    } else {
                        // Node 语义：参数不足时占位符**原样保留**
                        out.push_str("%s");
                    }
                }
                Some(&'d') => {
                    chars.next();
                    if arg_idx >= args.len() {
                        // Node 语义：参数不足时占位符原样保留
                        out.push_str("%d");
                        continue;
                    }
                    if arg_idx < args.len() {
                        out.push_str(&format_d(vm, args[arg_idx]));
                        arg_idx += 1;
                    }
                }
                Some(&'j') => {
                    // Node 语义：%j = JSON.stringify（Go 版用 String() 简化，
                    // 输出 `{ a: 1 }`；Node 22 实测 `{"a":1}`）
                    chars.next();
                    if arg_idx >= args.len() {
                        out.push_str("%j");
                        continue;
                    }
                    if arg_idx < args.len() {
                        let v = args[arg_idx];
                        arg_idx += 1;
                        let json =
                            vm.json_stringify_with_ops(v, Value::Undefined, Value::Undefined)?;
                        out.push_str(&vm.format_value(json));
                    }
                }
                Some(&'o') | Some(&'O') => {
                    chars.next();
                    if arg_idx < args.len() {
                        out.push_str(&inspect_value(vm, args[arg_idx]));
                        arg_idx += 1;
                    }
                }
                Some(&'%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    for extra in args.iter().skip(arg_idx) {
        out.push(' ');
        out.push_str(&inspect_value(vm, *extra));
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

/// `%d` 语义：数值先 `Int()` 截断（对齐 Go `numberValue.Int()`），否则按 String()。
fn format_d(vm: &Vm, v: Value) -> String {
    // Node 语义：`%d` 即 Number → String（**不截断**：`%d` 遇 3.5 输出 "3.5"；
    // Go 版按 Int() 截断成 "3"）
    match v.case() {
        ValueCase::Number(n) => crate::ops::js_number_to_string(n),
        _ => inspect_value(vm, v),
    }
}

/// `util.inspect(value)`：Node 22 形态（与 `console.log` 同源格式化器）。
///
/// 此前走 Go `Value.String()` 紧凑形态（字符串不带引号、对象 `{ a: 1 }`），
/// 与 Node 的 `'abc'` / `[ 1, 'x' ]` 不一致；现与 console 家族共用
/// `format_console_value`（选项参数暂不解析，登记后续项）。
fn inspect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let s = match args.first() {
        Some(v) => {
            let v = *v;
            vm.format_inspect_value(v)
        }
        None => "undefined".to_owned(),
    };
    Ok(Value::Object(vm.alloc_string(s)))
}

/// 值 → Go `String()` 等价的递归紧凑表示。
fn inspect_value(vm: &Vm, val: Value) -> String {
    match val.case() {
        ValueCase::Undefined | ValueCase::Null | ValueCase::Boolean(_) | ValueCase::Number(_) => {
            vm.format_value(val)
        }
        ValueCase::Object(r) => match vm.heap.get(r.index()) {
            Some(HeapObject::String(s)) => s.clone(),
            Some(HeapObject::BigInt(s)) => s.clone(),
            Some(HeapObject::Array { elements, .. }) => {
                if elements.is_empty() {
                    return "[]".to_owned();
                }
                let items: Vec<String> = elements.iter().map(|e| inspect_value(vm, *e)).collect();
                format!("[ {} ]", items.join(", "))
            }
            Some(HeapObject::Ordinary { .. }) => {
                let mut items: Vec<(String, Value)> = vm.own_entries(r.index());
                if items.is_empty() {
                    return "{}".to_owned();
                }
                items.sort_by(|a, b| a.0.cmp(&b.0));
                let strs: Vec<String> = items
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, inspect_value(vm, *v)))
                    .collect();
                format!("{{ {} }}", strs.join(", "))
            }
            Some(HeapObject::RegExp { pattern, flags }) => format!("/{pattern}/{flags}"),
            // Map/Set 格式化（Node 形态：`Map(1) { 1 => 'a' }` / `Set(2) { 1, 2 }`；
            // 空集合为 `Map(0) {}` / `Set(0) {}`）。此前落入 `[object Object]`。
            Some(HeapObject::Map { entries }) => {
                let is_set = vm.is_set_instance(val);
                let n = entries.len();
                if n == 0 {
                    return if is_set {
                        "Set(0) {}".to_owned()
                    } else {
                        "Map(0) {}".to_owned()
                    };
                }
                let items: Vec<String> = if is_set {
                    // Set 的键与值同存元素原值，取其一即可
                    entries.iter().map(|(_, v)| inspect_entry(vm, *v)).collect()
                } else {
                    entries
                        .iter()
                        .map(|(k, v)| {
                            format!("{} => {}", inspect_entry(vm, *k), inspect_entry(vm, *v))
                        })
                        .collect()
                };
                if is_set {
                    format!("Set({n}) {{ {} }}", items.join(", "))
                } else {
                    format!("Map({n}) {{ {} }}", items.join(", "))
                }
            }
            _ => "[object Object]".to_owned(),
        },
    }
}

/// `util.inspect` 的「条目级」表示：字符串加单引号（对齐 Node 的 `Map(1) { 1 => 'a' }`、
/// `Set(1) { 's' }`）；其余类型复用紧凑递归表示。
fn inspect_entry(vm: &Vm, val: Value) -> String {
    if let Some(r) = val.as_object() {
        if let Some(HeapObject::String(s)) = vm.heap.get(r.index()) {
            return format!("'{s}'");
        }
    }
    inspect_value(vm, val)
}

/// `util.types.isArray(v)`。
fn is_array(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined).case(),
        ValueCase::Object(rr) if matches!(vm.heap.get(rr.index()), Some(HeapObject::Array { .. }))
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isString(v)`。
fn is_string(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined).case(),
        ValueCase::Object(rr) if matches!(vm.heap.get(rr.index()), Some(HeapObject::String(_)))
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isNumber(v)`。
fn is_number(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined).case(),
        ValueCase::Number(_)
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isObject(v)`：对象形态（普通对象/数组等，排除字符串/函数）。
fn is_object(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = match args.first().copied().unwrap_or(Value::Undefined).case() {
        ValueCase::Object(rr) => matches!(
            vm.heap.get(rr.index()),
            Some(
                HeapObject::Ordinary { .. }
                    | HeapObject::Array { .. }
                    | HeapObject::Generator
                    | HeapObject::Promise { .. }
                    | HeapObject::Map { .. }
                    | HeapObject::RegExp { .. }
            )
        ),
        _ => false,
    };
    Ok(Value::Boolean(r))
}

/// `util.isDeepStrictEqual(a, b)`：复用 node:test 的深严格比较（与
/// `assert.deepStrictEqual` 同源）。
fn is_deep_strict_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let a = args.first().copied().unwrap_or(Value::Undefined);
    let b = args.get(1).copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(
        crate::builtins::test::asserts::deep_strict_equal(vm, a, b),
    ))
}

/// libuv/win32 的系统错误名映射（Node 22 Windows 实测口径：多数负 errno
/// 统一渲染为 `Unknown system error {code}`，仅个别广泛知名码有名）。
fn system_error_name(code: f64) -> String {
    match code as i64 {
        -40 => "EADDRINUSE".to_owned(),
        -111 => "ECONNREFUSED".to_owned(),
        -13 => "EACCES".to_owned(),
        -17 => "EEXIST".to_owned(),
        -21 => "EISDIR".to_owned(),
        -20 => "ENOTDIR".to_owned(),
        -39 => "ENOTEMPTY".to_owned(),
        -32 => "EPIPE".to_owned(),
        -22 => "EINVAL".to_owned(),
        other => format!("Unknown system error {other}"),
    }
}

/// `util.getSystemErrorName(code)`。
fn get_system_error_name(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let code = args.first().copied().unwrap_or(Value::Undefined);
    let name = system_error_name(vm.to_number_value(code));
    Ok(Value::Object(vm.alloc_string(name)))
}

/// `util.getSystemErrorMessage(code)`。
fn get_system_error_message(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let code = args.first().copied().unwrap_or(Value::Undefined);
    let message = system_error_name(vm.to_number_value(code));
    Ok(Value::Object(vm.alloc_string(message)))
}

/// `util.stripVTControlCharacters(str)`：剥除 ANSI 转义序列（CSI/OSC 简化面：
/// `ESC[`…终符 与 `ESC]`…BEL/ST）。
fn strip_vt_control_characters(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI：吞到 0x40..=0x7e 终符
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // OSC：吞到 BEL 或 ST（ESC \）
                while let Some(n) = chars.next() {
                    if n == '\u{07}' {
                        break;
                    }
                    if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

// ---- util.types 谓词 ----
fn types_is_promise(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let is_promise = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Promise { .. })));
    Ok(Value::Boolean(is_promise))
}

fn types_is_date(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let is_date = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.own_value(r.0 as usize, "_timeValue").map(|t| t.case()), Some(ValueCase::Number(_))));
    Ok(Value::Boolean(is_date))
}

fn types_is_regexp(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(vm.is_regexp_obj(v)))
}

fn types_is_error(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    // 沿原型链找 Error.prototype
    let mut proto = match v.case() {
        ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::Ordinary { proto, .. }) => *proto,
            _ => None,
        },
        _ => None,
    };
    let mut is_err = false;
    while let Some(p) = proto {
        if vm.error_prototype.is_some_and(|ep| ep.0 == p.0) {
            is_err = true;
            break;
        }
        proto = match vm.heap.get(p.0 as usize) {
            Some(HeapObject::Ordinary { proto, .. }) => *proto,
            _ => None,
        };
    }
    Ok(Value::Boolean(is_err))
}

fn types_is_map(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(vm.is_map_instance(v)))
}

fn types_is_set(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(vm.is_set_instance(v)))
}

fn types_is_typed_array(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let is_ta = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::TypedArray { .. })));
    Ok(Value::Boolean(is_ta))
}

fn types_is_async_function(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let is_async = matches!(v.case(), ValueCase::Object(r)
    if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::Closure { func_idx, .. })
            if vm
                .module_functions
                .get(*func_idx)
                .is_some_and(|t| t.is_async)
    ));
    Ok(Value::Boolean(is_async))
}

fn types_is_primitive(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = args.first().copied().unwrap_or(Value::Undefined);
    let is_prim = match v.case() {
        ValueCase::Object(r) => matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::String(_))
                | Some(HeapObject::BigInt(_))
                | Some(HeapObject::Symbol { .. })
        ),
        _ => true,
    };
    Ok(Value::Boolean(is_prim))
}

// ---- promisify / callbackify ----

/// `util.promisify(fn)`：回调风格 → Promise 风格。
///
/// 返回的函数以 `(...args, resolver)` 调用原函数；resolver 兑换承载在
/// 自身函数对象上的 `_promise`。sync 抛错按 Node 语义同步传播。
fn promisify(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let source = args.first().copied().unwrap_or(Value::Undefined);
    let wrapped = vm.alloc_native_fn("util.promisified");
    vm.set_native_fn_property(wrapped, "_src", source);
    register_handler(
        &mut vm.builtin_registry,
        "util",
        "promisified",
        promisified_call,
    );
    Ok(Value::Object(wrapped))
}

/// promisify 包装函数的调用体：`(…args) => Promise`。
fn promisified_call(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let source = vm.get_property(callee, "_src")?;
    let promise = vm.alloc_pending_promise();
    let resolver = vm.alloc_native_fn("util.promisify.resolver");
    vm.set_native_fn_property(resolver, "_promise", Value::Object(promise));
    register_handler(
        &mut vm.builtin_registry,
        "util",
        "promisify.resolver",
        promisify_resolver,
    );
    let mut call_args: Vec<Value> = args.to_vec();
    call_args.push(Value::Object(resolver));
    vm.invoke_callable(source, Value::Undefined, &call_args)?;
    Ok(Value::Object(promise))
}

/// promisify 的 resolver 回调：`(err, ...values)` → reject/resolve。
fn promisify_resolver(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let promise = match vm.get_property(callee, "_promise")?.case() {
        ValueCase::Object(r) => r,
        _ => return Ok(Value::Undefined),
    };
    let err = args.first().copied().unwrap_or(Value::Undefined);
    if !(err.is_undefined() || err.is_null()) {
        vm.reject_promise(promise, err)?;
        return Ok(Value::Undefined);
    }
    // Node 22 实测：多成功值也只兑现首个（cb(null, 1, 2) → resolve(1)）
    let value = args.get(1).copied().unwrap_or(Value::Undefined);
    vm.fulfill_promise(promise, value)?;
    Ok(Value::Undefined)
}

/// `util.callbackify(fn)`：Promise 风格 → 回调风格。
fn callbackify(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let source = args.first().copied().unwrap_or(Value::Undefined);
    let wrapped = vm.alloc_native_fn("util.callbackified");
    vm.set_native_fn_property(wrapped, "_src", source);
    register_handler(
        &mut vm.builtin_registry,
        "util",
        "callbackified",
        callbackified_call,
    );
    Ok(Value::Object(wrapped))
}

/// callbackify 包装函数的调用体：`(…args, cb)`。
fn callbackified_call(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let source = vm.get_property(callee, "_src")?;
    if args.is_empty() {
        return Err(vm.type_error("callbackify: the wrapped function expects a callback"));
    }
    let cb = args[args.len() - 1];
    let fn_args = &args[..args.len() - 1];
    let result = vm.invoke_callable(source, Value::Undefined, fn_args);
    match result {
        Ok(ret) => {
            if matches!(ret.case(), ValueCase::Object(r)
                if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Promise { .. })))
            {
                // Promise 结果：经统一方法分派挂原生回调（Promise 实例的
                // `then` 在 CALL_METHOD 特判，属性读取路径拿不到）
                let ok = vm.alloc_native_fn("util.callbackify.onFulfilled");
                vm.set_native_fn_property(ok, "_cb", cb);
                let err = vm.alloc_native_fn("util.callbackify.onRejected");
                vm.set_native_fn_property(err, "_cb", cb);
                register_handler(
                    &mut vm.builtin_registry,
                    "util",
                    "callbackify.onFulfilled",
                    callbackify_on_fulfilled,
                );
                register_handler(
                    &mut vm.builtin_registry,
                    "util",
                    "callbackify.onRejected",
                    callbackify_on_rejected,
                );
                vm.call_method_dispatch(ret, "then", &[Value::Object(ok), Value::Object(err)], 0)?;
            } else {
                vm.invoke_callable(cb, Value::Undefined, &[Value::Null, ret])?;
            }
            Ok(Value::Undefined)
        }
        Err(err) => {
            let thrown = match err {
                VmError::Thrown(v) => v,
                other => Value::Object(vm.alloc_string(format!("{other:?}"))),
            };
            vm.invoke_callable(cb, Value::Undefined, &[thrown])?;
            Ok(Value::Undefined)
        }
    }
}

fn callbackify_on_fulfilled(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let cb = vm.get_property(callee, "_cb")?;
    let value = args.first().copied().unwrap_or(Value::Undefined);
    vm.invoke_callable(cb, Value::Undefined, &[Value::Null, value])?;
    Ok(Value::Undefined)
}

fn callbackify_on_rejected(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let callee = crate::builtins::pending_callee();
    let cb = vm.get_property(callee, "_cb")?;
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    vm.invoke_callable(cb, Value::Undefined, &[reason])?;
    Ok(Value::Undefined)
}

// ---- legacy isX 族 ----
fn legacy_first(args: &[Value]) -> Value {
    args.first().copied().unwrap_or(Value::Undefined)
}

fn legacy_is_array(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let ok = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Array { .. })));
    Ok(Value::Boolean(ok))
}

fn legacy_is_boolean(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(matches!(
        legacy_first(args).case(),
        ValueCase::Boolean(_)
    )))
}

fn legacy_is_null(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(legacy_first(args) == Value::Null))
}

fn legacy_is_null_or_undefined(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    Ok(Value::Boolean(v == Value::Null || v.is_undefined()))
}

fn legacy_is_number(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(matches!(
        legacy_first(args).case(),
        ValueCase::Number(_)
    )))
}

fn legacy_is_string(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    Ok(Value::Boolean(vm.is_string_value(v)))
}

fn legacy_is_symbol(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let ok = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.heap.get(r.0 as usize), Some(HeapObject::Symbol { .. })));
    Ok(Value::Boolean(ok))
}

fn legacy_is_undefined(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(legacy_first(args).is_undefined()))
}

fn legacy_is_object(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let ok = match v.case() {
        ValueCase::Object(r) => !matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::String(_))
                | Some(HeapObject::BigInt(_))
                | Some(HeapObject::Symbol { .. })
        ),
        _ => false,
    };
    Ok(Value::Boolean(ok))
}

fn legacy_is_function(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(vm.is_callable_value(legacy_first(args))))
}

fn legacy_is_buffer(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let ok = matches!(v.case(), ValueCase::Object(r)
        if vm.has_own_slot(r.0 as usize, "_isBuffer"));
    Ok(Value::Boolean(ok))
}

fn legacy_is_primitive(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let is_prim = match v.case() {
        ValueCase::Object(r) => matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::String(_))
                | Some(HeapObject::BigInt(_))
                | Some(HeapObject::Symbol { .. })
        ),
        _ => true,
    };
    Ok(Value::Boolean(is_prim))
}

fn legacy_is_regexp(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    Ok(Value::Boolean(vm.is_regexp_obj(legacy_first(args))))
}

fn legacy_is_date(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let v = legacy_first(args);
    let ok = matches!(v.case(), ValueCase::Object(r)
        if matches!(vm.own_value(r.0 as usize, "_timeValue").map(|t| t.case()), Some(ValueCase::Number(_))));
    Ok(Value::Boolean(ok))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = format;
        let _: crate::builtins::BuiltinHandler = inspect;
        let _: crate::builtins::BuiltinHandler = is_array;
        let _: crate::builtins::BuiltinHandler = is_string;
        let _: crate::builtins::BuiltinHandler = is_number;
        let _: crate::builtins::BuiltinHandler = is_object;
    }
}
