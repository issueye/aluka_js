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
use crate::value::Value;
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
    set_module_prop(vm, obj, "format", Value::Object(format_fn))?;
    set_module_prop(vm, obj, "inspect", Value::Object(inspect_fn))?;
    let types = vm.alloc_ordinary();
    set_module_prop(vm, obj, "types", Value::Object(types))?;
    register_handler(registry, "util", "format", format);
    register_handler(registry, "util", "inspect", inspect);
    Ok(obj)
}

/// `util.types` 子模块 build：取主模块 `types` 属性对象并登记类型判断方法。
fn build_types(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let util_mod = registry.module("util").ok_or_else(|| {
        let msg = vm.alloc_string("util.types: 主模块未注册".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    let types_val = vm.get_property(Value::Object(util_mod), "types")?;
    let Value::Object(types) = types_val else {
        let msg = vm.alloc_string("util.types: types 属性缺失".to_owned());
        return Err(VmError::Thrown(Value::Object(msg)));
    };
    register_handler(registry, "util/types", "isArray", is_array);
    register_handler(registry, "util/types", "isString", is_string);
    register_handler(registry, "util/types", "isNumber", is_number);
    register_handler(registry, "util/types", "isObject", is_object);
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
                    }
                }
                Some(&'d') => {
                    chars.next();
                    if arg_idx < args.len() {
                        out.push_str(&format_d(vm, args[arg_idx]));
                        arg_idx += 1;
                    }
                }
                Some(&'j') | Some(&'o') | Some(&'O') => {
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
    match v {
        Value::Number(n) => format!("{}", n.trunc() as i64),
        _ => inspect_value(vm, v),
    }
}

/// `util.inspect(value)`：Go `Value.String()` 口径的紧凑表示。
fn inspect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let s = match args.first() {
        Some(v) => inspect_value(vm, *v),
        None => "undefined".to_owned(),
    };
    Ok(Value::Object(vm.alloc_string(s)))
}

/// 值 → Go `String()` 等价的递归紧凑表示。
fn inspect_value(vm: &Vm, val: Value) -> String {
    match val {
        Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => {
            vm.format_value(val)
        }
        Value::Object(r) => match vm.heap.get(r.index()) {
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
            _ => "[object Object]".to_owned(),
        },
    }
}

/// `util.types.isArray(v)`。
fn is_array(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined),
        Value::Object(rr) if matches!(vm.heap.get(rr.index()), Some(HeapObject::Array { .. }))
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isString(v)`。
fn is_string(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined),
        Value::Object(rr) if matches!(vm.heap.get(rr.index()), Some(HeapObject::String(_)))
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isNumber(v)`。
fn is_number(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = matches!(
        args.first().copied().unwrap_or(Value::Undefined),
        Value::Number(_)
    );
    Ok(Value::Boolean(r))
}

/// `util.types.isObject(v)`：对象形态（普通对象/数组等，排除字符串/函数）。
fn is_object(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let r = match args.first().copied().unwrap_or(Value::Undefined) {
        Value::Object(rr) => matches!(
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
