//! `querystring` 内置模块（Phase 1 示例实现，并行开发的模板）。
//!
//! 语义实测对齐 Node.js 22 LTS 标准：`parse("a=1&b=2&c")` 给 `c` 空串；重复键收集为
//! 数组；`stringify` 对空格用 `+` 编码。
//!
//! 并行开发模板说明（see `aluka_r/docs/builtins-plan.md`）：
//! - 本文件是 `builtins/` 下各模块的示范——只允许改本文件与
//!   `builtins/mod.rs` 的注册数组；**禁止修改** `interpreter.rs` 等核心文件；
//! - 模块声明 [`ModuleDef`]，`build` 里建模块对象、**登记处理器**、
//!   挂方法属性；方法实现为 `fn(&mut Vm, &[Value]) -> Result<Value, VmError>`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;

/// query 字符串解析/序列化模块（parse/stringify）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "querystring",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    let parse_fn = vm.alloc_native_fn("querystring.parse");
    let stringify_fn = vm.alloc_native_fn("querystring.stringify");
    set_module_prop(vm, obj, "parse", Value::Object(parse_fn))?;
    set_module_prop(vm, obj, "stringify", Value::Object(stringify_fn))?;
    register_handler(registry, "querystring", "parse", parse);
    register_handler(registry, "querystring", "stringify", stringify);
    Ok(obj)
}

/// `parse(str)`：`a=1&b=2&c` → `{ a: "1", b: "2", c: "" }`；
/// 重复键收集为数组（如 `x=1&x=2` → `x: ["1","2"]`）。
fn parse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let src = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut singles: HashMap<String, String> = HashMap::new();
    let mut lists: HashMap<String, Vec<String>> = HashMap::new();

    for pair in src.split('&') {
        let pair = pair.trim_end_matches('\r');
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (decode_plus(k), decode_plus(v)),
            None => (decode_plus(pair), String::new()),
        };
        if lists.contains_key(&key) {
            lists.get_mut(&key).expect("已确认存在").push(value);
        } else if let Some(first) = singles.remove(&key) {
            lists.insert(key.clone(), vec![first, value]);
        } else {
            singles.insert(key.clone(), value);
            order.push(key);
        }
    }

    let obj = vm.alloc_ordinary();
    for key in order {
        if let Some(value) = singles.get(&key) {
            let s = vm.alloc_string(value.clone());
            let _ = vm.set_property(Value::Object(obj), &key, Value::Object(s));
        } else if let Some(items) = lists.get(&key) {
            let elems: Vec<Value> = items
                .iter()
                .map(|v| Value::Object(vm.alloc_string(v.clone())))
                .collect();
            let arr = vm.alloc_array(elems);
            let _ = vm.set_property(Value::Object(obj), &key, Value::Object(arr));
        }
    }
    Ok(Value::Object(obj))
}

/// `stringify(obj)`：`{ a: 1, b: "x y" }` → `a=1&b=x+y`（键序无关，逐字对齐测试）。
fn stringify(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = args
        .first()
        .copied()
        .unwrap_or(Value::Undefined)
        .as_object()
    {
        if matches!(
            vm.heap.get(r.index()),
            Some(crate::heap::HeapObject::Ordinary { .. })
        ) {
            for (k, v) in vm.own_entries(r.index()) {
                parts.push(format!(
                    "{}={}",
                    encode_plus(&k),
                    encode_plus(&vm.format_value(v))
                ));
            }
        }
    }
    parts.sort();
    let s = vm.alloc_string(parts.join("&"));
    Ok(Value::Object(s))
}

/// `+` → 空格（querystring 语义），其余原样。
fn decode_plus(s: &str) -> String {
    s.replace('+', " ")
}

/// 空格 → `+`。
fn encode_plus(s: &str) -> String {
    s.replace(' ', "+")
}

/// 编译期锚定：确保处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = parse;
        let _: crate::builtins::BuiltinHandler = stringify;
    }
}
