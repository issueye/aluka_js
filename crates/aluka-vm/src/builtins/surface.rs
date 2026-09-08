//! 内建构造器「原型方法面」属性挂载（M2.4 express 依赖树补全）。
//!
//! 真实包（object-inspect/qs/side-channel 等）在模块顶层把
//! `String.prototype.match` 这类方法**取属性存槽**再 `.call` 调用；
//! `.prototype` 缺失时取属性即 `undefined`，模块加载直接抛
//! TypeError（M2.4 实测：object-inspect 读 `String.prototype.match`
//! 崩 `Cannot read properties of undefined (reading 'match')`）。
//!
//! 本模块只保证原型对象与属性**存在**（挂 NativeFn 占位）：
//! - `.call`/`.apply` 调用形态由解释器通用调用协议消化并转发到方法实体；
//! - `Function.prototype.toString` 提供真实 handler（interpreter 侧注册）；
//! - 实例方法的指令级语义在 CALL_METHOD 大链按 receiver 类型特判实现，
//!   不依赖本面的实体。
//!
//! 真实 handler 的逐个补全属于对象面大工程（后续里程碑），此处不为
//! 未实现方法注册 handler——调用缺位方法会抛 "not a function"，保证
//! 语义可见而非静默。

use super::{BuiltinRegistry, register_handler};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// 原型方法面注册（幂等；`Vm::new` 的 register_all 中调用一次）。
pub fn register_surface(vm: &mut Vm, registry: &mut BuiltinRegistry) {
    // Object.prototype（单例已由 Vm 初始化，面只做补挂）
    let obj_proto = match vm.object_prototype {
        Some(p) => p,
        None => {
            let p = vm.alloc_ordinary_with_exact_proto(None);
            vm.object_prototype = Some(p);
            p
        }
    };
    for m in [
        "toString",
        "valueOf",
        "hasOwnProperty",
        "propertyIsEnumerable",
        "isPrototypeOf",
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "toLocaleString",
        "constructor",
    ] {
        let f = vm.alloc_native_fn(&format!("Object.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(obj_proto), m, Value::Object(f));
    }
    // Object.prototype 真 handler（`.call`/`.apply` 形态：真实包把方法存槽
    // 再 `$hasOwnProperty.call(obj, k)` 调用——占位会抛 not-a-function）
    register_handler(
        registry,
        "Object.prototype",
        "hasOwnProperty",
        obj_has_own_prop,
    );
    register_handler(registry, "Object.prototype", "toString", obj_to_string_tag);
    register_handler(registry, "Object.prototype", "valueOf", obj_value_of);
    register_handler(
        registry,
        "Object.prototype",
        "propertyIsEnumerable",
        obj_prop_is_enum,
    );
    register_handler(
        registry,
        "Object.prototype",
        "isPrototypeOf",
        obj_is_proto_of,
    );
    register_handler(
        registry,
        "Object.prototype",
        "toLocaleString",
        obj_to_string_tag,
    );

    // 字符串实例方法面
    let str_p = str_proto(vm);
    for m in [
        "toString",
        "valueOf",
        "charAt",
        "charCodeAt",
        "codePointAt",
        "at",
        "concat",
        "endsWith",
        "includes",
        "indexOf",
        "lastIndexOf",
        "localeCompare",
        "match",
        "matchAll",
        "normalize",
        "padEnd",
        "padStart",
        "repeat",
        "replace",
        "replaceAll",
        "search",
        "slice",
        "split",
        "startsWith",
        "substring",
        "substr",
        "toLocaleLowerCase",
        "toLocaleUpperCase",
        "toLowerCase",
        "toUpperCase",
        "trim",
        "trimEnd",
        "trimStart",
        "trimLeft",
        "trimRight",
        "anchor",
        "big",
        "blink",
        "bold",
        "fixed",
        "fontcolor",
        "fontsize",
        "italics",
        "link",
        "small",
        "strike",
        "sub",
        "sup",
        "isWellFormed",
        "toWellFormed",
    ] {
        let f = vm.alloc_native_fn(&format!("String.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(str_p), m, Value::Object(f));
        // 除 length（属性）外注册真转发 handler（`.call` 形态经注册表分派）
        if m != "length" {
            register_handler(registry, "String.prototype", m, str_method_dispatch);
        }
    }

    // Symbol 实例方法面（toString/valueOf/description）
    let sym_p = symbol_proto(vm);
    for m in ["toString", "valueOf", "description", "for", "keyFor"] {
        let f = vm.alloc_native_fn(&format!("Symbol.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(sym_p), m, Value::Object(f));
    }

    // Boolean / Number 实例方法面
    let bool_p = bool_proto(vm);
    for m in ["toString", "valueOf"] {
        let f = vm.alloc_native_fn(&format!("Boolean.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(bool_p), m, Value::Object(f));
    }
    let num_p = num_proto(vm);
    for m in [
        "toString",
        "valueOf",
        "toFixed",
        "toExponential",
        "toPrecision",
        "toLocaleString",
    ] {
        let f = vm.alloc_native_fn(&format!("Number.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(num_p), m, Value::Object(f));
    }

    // Function.prototype：toString 真实 handler（interpreter 侧注册），
    // call/apply/bind 属性占位（调用形态经解释器通用协议消化）
    let fn_p = fn_proto(vm);
    for m in [
        "call",
        "apply",
        "bind",
        "toString",
        "name",
        "length",
        "constructor",
    ] {
        let f = vm.alloc_native_fn(&format!("Function.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(fn_p), m, Value::Object(f));
    }
    register_handler(
        registry,
        "Function.prototype",
        "toString",
        fn_proto_to_string,
    );
    register_handler(registry, "Function.prototype", "call", fn_proto_call_apply);
    register_handler(registry, "Function.prototype", "apply", fn_proto_call_apply);
    register_handler(registry, "Function.prototype", "bind", fn_proto_bind);
    register_handler(
        registry,
        "Function.prototype.bound",
        "call",
        bound_fn_invoke,
    );

    // RegExp 实例方法面
    let re_p = regexp_proto(vm);
    for m in [
        "exec",
        "test",
        "toString",
        "compile",
        "source",
        "flags",
        "global",
        "ignoreCase",
        "multiline",
        "dotAll",
        "sticky",
        "unicode",
        "lastIndex",
    ] {
        let f = vm.alloc_native_fn(&format!("RegExp.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(re_p), m, Value::Object(f));
    }
    register_handler(registry, "RegExp.prototype", "exec", regexp_exec_dispatch);
    register_handler(registry, "RegExp.prototype", "test", regexp_test_dispatch);
    register_handler(
        registry,
        "RegExp.prototype",
        "toString",
        regexp_to_string_dispatch,
    );

    // Array 实例方法面
    let arr_p = array_proto(vm);
    for m in [
        "toString",
        "toLocaleString",
        "concat",
        "copyWithin",
        "entries",
        "every",
        "fill",
        "filter",
        "find",
        "findIndex",
        "findLast",
        "findLastIndex",
        "flat",
        "flatMap",
        "forEach",
        "includes",
        "indexOf",
        "join",
        "keys",
        "lastIndexOf",
        "map",
        "pop",
        "push",
        "reduce",
        "reduceRight",
        "reverse",
        "shift",
        "slice",
        "some",
        "sort",
        "splice",
        "toReversed",
        "toSorted",
        "toSpliced",
        "unshift",
        "values",
        "with",
        "at",
        "constructor",
    ] {
        let f = vm.alloc_native_fn(&format!("Array.prototype.{m}"));
        let _ = vm.define_proto_method(Value::Object(arr_p), m, Value::Object(f));
    }

    // Set/Map/WeakSet/WeakMap/WeakRef 容器原型方法面（属性存在性）
    let cont_p = container_proto(vm);
    for m in [
        "add",
        "has",
        "delete",
        "clear",
        "forEach",
        "get",
        "set",
        "keys",
        "values",
        "entries",
        "deref",
        "size",
        "constructor",
    ] {
        let f = vm.alloc_native_fn(&format!("proto.{m}"));
        let _ = vm.define_proto_method(Value::Object(cont_p), m, Value::Object(f));
    }
}

macro_rules! proto_getter {
    ($name:ident, $field:ident) => {
        pub(crate) fn $name(vm: &mut Vm) -> ObjectRef {
            if let Some(p) = vm.$field {
                return p;
            }
            let p = vm.alloc_ordinary_with_proto(None);
            vm.$field = Some(p);
            p
        }
    };
}

proto_getter!(str_proto, str_proto);
proto_getter!(symbol_proto, symbol_proto);
proto_getter!(bool_proto, bool_proto);
proto_getter!(num_proto, num_proto);
proto_getter!(fn_proto, fn_proto);
proto_getter!(regexp_proto, regexp_proto);
proto_getter!(array_proto, array_proto_surface);
proto_getter!(container_proto, container_proto);

/// `Function.prototype.call/apply`（真实 handler）：`this`（current_receiver）
/// 是被调函数，实参首位为 thisArg——转发 `invoke_callable`（call-bind-apply-
/// helpers 的 `bind.call($call, $apply)` 形态即依赖此实现）。
fn fn_proto_call_apply(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = super::pending_native_name();
    let this_fn = super::current_receiver();
    let this_arg = args.first().copied().unwrap_or(Value::Undefined);
    let call_args: Vec<Value> = if name.ends_with(".call") {
        if args.len() <= 1 {
            Vec::new()
        } else {
            args[1..].to_vec()
        }
    } else {
        // apply：第二实参为参数数组
        args.get(1)
            .map(|a| vm.to_array_values(*a))
            .unwrap_or_default()
    };
    vm.invoke_callable(this_fn, this_arg, &call_args)
}

/// `Function.prototype.bind`：返回绑定函数（NativeFn 实例，目标/this/预设参
/// 存自有属性；调用经 [`bound_fn_invoke`] 转发）。
fn fn_proto_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this_fn = super::current_receiver();
    let b = vm.alloc_native_fn("Function.prototype.bound");
    vm.set_native_fn_property(b, "_target", this_fn);
    let this_arg = args.first().copied().unwrap_or(Value::Undefined);
    vm.set_native_fn_property(b, "_this", this_arg);
    let preset = if args.len() <= 1 {
        Vec::new()
    } else {
        args[1..].to_vec()
    };
    let preset_arr = vm.alloc_array(preset);
    vm.set_native_fn_property(b, "_args", Value::Object(preset_arr));
    Ok(Value::Object(b))
}

/// 绑定函数调用：目标函数 + 绑定 this + 预设参 + 调用实参。
fn bound_fn_invoke(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let b = super::current_receiver();
    let target = match b {
        Value::Object(r) => vm
            .get_native_fn_property(r, "_target")
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    };
    let this_arg = match b {
        Value::Object(r) => vm
            .get_native_fn_property(r, "_this")
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    };
    let mut call_args: Vec<Value> = match b {
        Value::Object(r) => match vm.get_native_fn_property(r, "_args") {
            Some(Value::Object(arr)) => vm.to_array_values(Value::Object(arr)),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    call_args.extend_from_slice(args);
    vm.invoke_callable(target, this_arg, &call_args)
}

/// `Object.prototype.hasOwnProperty.call(obj, key)`：自有属性判定。
fn obj_has_own_prop(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    let key = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let has = match this {
        Value::Object(r) => vm.has_own_slot(r.0 as usize, &key),
        _ => false,
    };
    Ok(Value::Boolean(has))
}

/// `Object.prototype.toString.call(v)`：`[object Tag]`。
fn obj_to_string_tag(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    use crate::value::Value as V;
    let this = super::current_receiver();
    let tag = match this {
        V::Undefined => "Undefined",
        V::Null => "Null",
        V::Boolean(_) => "Boolean",
        V::Number(_) => "Number",
        V::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::String(_)) => "String",
            Some(HeapObject::Array { .. }) => "Array",
            Some(HeapObject::RegExp { .. }) => "RegExp",
            Some(HeapObject::Closure { .. })
            | Some(HeapObject::NativeFn { .. })
            | Some(HeapObject::NativeCtor { .. }) => "Function",
            Some(HeapObject::BigInt(_)) => "BigInt",
            Some(HeapObject::Map { .. }) => "Map",
            Some(HeapObject::Promise { .. }) => "Promise",
            Some(HeapObject::Generator) => "Generator",
            _ => "Object",
        },
    };
    Ok(Value::Object(vm.alloc_string(format!("[object {tag}]"))))
}

/// `Object.prototype.valueOf.call(v)`：原样返回。
fn obj_value_of(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(super::current_receiver())
}

/// `Object.prototype.propertyIsEnumerable.call(obj, key)`。
fn obj_prop_is_enum(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    let key = args
        .first()
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let en = match this {
        Value::Object(r) => {
            // 自有属性且不在不可枚举集合
            vm.has_own_slot(r.0 as usize, &key)
                && !matches!(
                    vm.heap.get(r.0 as usize),
                    Some(HeapObject::Ordinary { non_enum, .. }) if non_enum.contains(&key)
                )
        }
        _ => false,
    };
    Ok(Value::Boolean(en))
}

/// `Object.prototype.isPrototypeOf.call(proto, probe)`：沿 probe 原型链查找。
fn obj_is_proto_of(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    let probe = args.first().copied().unwrap_or(Value::Undefined);
    let Value::Object(target) = probe else {
        return Ok(Value::Boolean(false));
    };
    let Value::Object(proto) = this else {
        return Ok(Value::Boolean(false));
    };
    let mut cur = Some(target);
    for _ in 0..64 {
        let Some(c) = cur.take() else { break };
        if c == proto {
            return Ok(Value::Boolean(true));
        }
        cur = match vm.heap.get(c.0 as usize) {
            Some(HeapObject::Ordinary { proto: p, .. }) => *p,
            Some(HeapObject::Array { proto: p, .. }) => *p,
            Some(HeapObject::NativeCtor { properties, .. }) => {
                properties.get("prototype").and_then(|v| match v {
                    Value::Object(r) => Some(*r),
                    _ => None,
                })
            }
            Some(HeapObject::Closure { properties, .. })
            | Some(HeapObject::NativeFn { properties, .. }) => {
                properties.get("prototype").and_then(|v| match v {
                    Value::Object(r) => Some(*r),
                    _ => None,
                })
            }
            _ => None,
        };
    }
    Ok(Value::Boolean(false))
}

/// `String.prototype.X.call(thisStr, ...)`：转发既有字符串方法实现。
fn str_method_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let full = super::pending_native_name();
    let name = full
        .rsplit("String.prototype.")
        .next()
        .unwrap_or(&full)
        .to_owned();
    let this = super::current_receiver();
    let text = match &this {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::String(t)) => t.clone(),
            _ => vm.format_value(this),
        },
        Value::Undefined | Value::Null => {
            let msg = vm.alloc_string(format!(
                "Cannot convert undefined or null to object (String.prototype.{name})"
            ));
            return Err(VmError::Thrown(Value::Object(msg)));
        }
        _ => vm.format_value(this),
    };
    match vm.call_string_method(&name, args, &text) {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(e),
        None => {
            let msg = vm.alloc_string(format!("String.prototype.{name} is not a function"));
            Err(VmError::Thrown(Value::Object(msg)))
        }
    }
}

/// `RegExp.prototype.exec.call(re, str)`。
fn regexp_exec_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let re = super::current_receiver();
    let subject = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    match vm.regexp_exec(re, &subject)? {
        Some(Value::Object(arr)) => Ok(Value::Object(arr)),
        _ => Ok(Value::Null),
    }
}

/// `RegExp.prototype.test.call(re, str)`。
fn regexp_test_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let re = super::current_receiver();
    let subject = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    Ok(Value::Boolean(vm.regexp_exec(re, &subject)?.is_some()))
}

/// `RegExp.prototype.toString.call(re)`。
fn regexp_to_string_dispatch(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let re = super::current_receiver();
    let s = vm.format_value(re);
    Ok(Value::Object(vm.alloc_string(s)))
}

/// `Function.prototype.toString`（真实 handler）：函数对象文本面，无源码
/// 映射时返回 `function name() { [native code] }` 占位；非函数 this 抛
/// TypeError（对齐 Node）。
fn fn_proto_to_string(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    let text = match this {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::Closure { func_idx, .. }) => {
                let name = vm
                    .module_functions
                    .get(*func_idx)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                format!("function {name}() {{ [native code] }}")
            }
            Some(HeapObject::NativeFn { name, .. }) => {
                let short = name.rsplit('.').next().unwrap_or(name);
                format!("function {short}() {{ [native code] }}")
            }
            Some(HeapObject::NativeCtor { name, .. }) => {
                format!("function {name}() {{ [native code] }}")
            }
            _ => {
                let msg = vm.alloc_string(
                    "Function.prototype.toString requires 'this' be a Function".to_owned(),
                );
                return Err(VmError::Thrown(Value::Object(msg)));
            }
        },
        _ => {
            let msg = vm.alloc_string(
                "Function.prototype.toString requires 'this' be a Function".to_owned(),
            );
            return Err(VmError::Thrown(Value::Object(msg)));
        }
    };
    Ok(Value::Object(vm.alloc_string(text)))
}
