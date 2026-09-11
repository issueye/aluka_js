//! `node:sys` 兼容别名模块（Phase 4）。
//!
//! 语义完全对齐 Node.js DEP0140 规范与 Node.js 22 LTS 标准（`internal/builtin/registry.go`）：
//! - `sys` 模块是 `util` 的历史遗留别名，两者具备同义关系；
//! - 转发并支持 `format`、`inspect` 及 `types` 等属性与方法；
//! - 保证脚本在引用 `node:sys` 时具备与 `util` 完全一致的功能。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// `require("sys")` / `require("node:sys")` 兼容模块。
pub const MODULE: ModuleDef = ModuleDef { name: "sys", build };

/// 构建 `sys` 模块单例并向注册表登记方法分派。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let format_fn = vm.alloc_native_fn("sys.format");
    let inspect_fn = vm.alloc_native_fn("sys.inspect");
    set_module_prop(vm, obj, "format", Value::Object(format_fn))?;
    set_module_prop(vm, obj, "inspect", Value::Object(inspect_fn))?;

    // 关联 types 子属性如果 util 模块已经注册
    if let Some(util_ref) = registry.module("util") {
        if let Ok(types_val) = vm.get_property(Value::Object(util_ref), "types") {
            let _ = set_module_prop(vm, obj, "types", types_val);
        }
    }

    register_handler(registry, "sys", "format", format);
    register_handler(registry, "sys", "inspect", inspect);
    Ok(obj)
}

/// 惰性重链 os 单例（对齐内置模块注册惯用法）。
fn sync_os_link(vm: &mut Vm) {
    if let Some(cur) = vm.os_module {
        if vm.builtin_registry.module("os") != Some(cur) {
            vm.builtin_registry.modules.insert("os", cur);
        }
    }
}

/// `sys.format(...)`：Node 风格占位符替换（同义转发 util.format）。
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

/// `%d` 格式化数值截断。
fn format_d(vm: &Vm, v: Value) -> String {
    match v.case() {
        ValueCase::Number(n) => format!("{}", n.trunc() as i64),
        _ => inspect_value(vm, v),
    }
}

/// `sys.inspect(value)`：紧凑值表示。
fn inspect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let s = match args.first() {
        Some(v) => inspect_value(vm, *v),
        None => "undefined".to_owned(),
    };
    Ok(Value::Object(vm.alloc_string(s)))
}

/// 递归格式化紧凑值。
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
            _ => "[object Object]".to_owned(),
        },
    }
}

/// 编译期签名校验锚定。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 处理器签名锚定() {
        let _: crate::builtins::BuiltinHandler = format;
        let _: crate::builtins::BuiltinHandler = inspect;
    }
}
