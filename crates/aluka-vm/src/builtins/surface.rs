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
    // 构造器 prototype 统一：Vm::new 预建的 ctor 原型是无方法面的空对象，
    // 方法面挂在本模块原型单例上——把 ctor 的 `prototype` 属性重定向到
    // 单例（RegExp/Set/Map/Array：真实包取 `RegExp.prototype.test` 等存槽）
    for (ctor_field, proto) in [
        (vm.regexp_ctor, regexp_proto(vm)),
        (vm.set_ctor, container_proto(vm)),
        (vm.map_ctor, container_proto(vm)),
        (vm.array_ctor, array_proto(vm)),
    ] {
        if let Some(c) = ctor_field
            && let Some(crate::heap::HeapObject::NativeCtor { properties, .. }) =
                vm.heap.get_mut(c.0 as usize)
        {
            properties.insert("prototype".to_owned(), Value::Object(proto));
        }
    }
    // RegExp 字面量对象的原型字段统一（Vm::new 预建的 regexp_prototype 是
    // 无方法面的空对象——字面量 /re/ 的 `.test/.exec` 经原型链读取依赖
    // 本模块方法面单例）
    vm.regexp_prototype = Some(regexp_proto(vm));
    // 数组实例原型统一（alloc_array 用 array_prototype 字段——ctor 的
    // prototype 已重定向到方法面单例，实例链必须同源否则
    // `[] instanceof Array` 失效）
    vm.array_prototype = Some(array_proto(vm));

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
        register_handler(registry, "Number.prototype", m, num_method_dispatch);
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
    // `constructor` → 真 Function 构造器（同上：占位会令 `f.constructor.name` 错）
    if let Value::Object(fc) = vm.resolve_global("Function") {
        let _ = vm.set_property(Value::Object(fn_p), "constructor", Value::Object(fc));
    }
    register_handler(registry, "Function.prototype", "call", fn_proto_call_apply);
    register_handler(registry, "Function.prototype", "apply", fn_proto_call_apply);
    register_handler(registry, "Function.prototype", "bind", fn_proto_bind);
    register_handler(registry, "Function.prototype", "bound", bound_fn_invoke);

    // RegExp 实例方法面
    let re_p = regexp_proto(vm);
    if let Some(rc) = vm.regexp_ctor {
        let _ = vm.define_proto_method(Value::Object(re_p), "constructor", Value::Object(rc));
    }
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
        // 真 handler：真实包（express application）`Array.prototype.slice.call`
        // 形态——占位会抛 not-a-function
        register_handler(registry, "Array.prototype", m, array_method_dispatch);
    }
    // `constructor` 指向**真构造器**：上方占位使 `[].constructor.name` 得
    // "Array.prototype.constructor"（Node 为 "Array"），且 `x.constructor === Array`
    // 是生态里常见的鸭子判定（object-inspect 等）。构造器单例在 register_all
    // 之前已由 Vm::new 建好，此处可安全覆盖。
    if let Some(c) = vm.array_ctor {
        let _ = vm.set_property(Value::Object(arr_p), "constructor", Value::Object(c));
    }
    if let Some(c) = vm.object_ctor {
        let _ = vm.set_property(Value::Object(obj_proto), "constructor", Value::Object(c));
    }

    // Array 静态方法（from / of）——生成语料实测缺失（`Array.from is not a function`）
    if let Some(ctor) = vm.array_ctor {
        for st in ["from", "of"] {
            let f = vm.alloc_native_fn(&format!("Array.{st}"));
            let _ = vm.set_property(Value::Object(ctor), st, Value::Object(f));
            register_handler(registry, "Array", st, array_static_dispatch);
        }
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

    // === Symbol.iterator 注册 ===
    // 为 String / Array / Map / Set 原型挂 `[Symbol.iterator]` 方法，
    // 使 `for...of` 的 GetIterator fallback 路径（Symbol.iterator 属性查找 +
    // 函数调用）取得迭代器。Array 还保留 GetIterator 直接快速路径作为优化。
    let iter_sym = vm.well_known_symbol("iterator");
    let iter_mkey = match iter_sym {
        Value::Object(r) => crate::symbol::mangled_key(r),
        _ => unreachable!("well_known_symbol returns symbol object"),
    };

    // String.prototype[Symbol.iterator]
    let str_p = str_proto(vm);
    let f = vm.alloc_native_fn("String.prototype.Symbol.iterator");
    let _ = vm.set_property(Value::Object(str_p), &iter_mkey, Value::Object(f));
    register_handler(
        registry,
        "String.prototype",
        "Symbol.iterator",
        str_iter_handler,
    );

    // Array.prototype[Symbol.iterator] → 委托给 values()
    let arr_p = array_proto(vm);
    let f = vm.alloc_native_fn("Array.prototype.Symbol.iterator");
    let _ = vm.set_property(Value::Object(arr_p), &iter_mkey, Value::Object(f));
    register_handler(
        registry,
        "Array.prototype",
        "Symbol.iterator",
        arr_iter_handler,
    );

    // Map/Set 共用 container_proto：[Symbol.iterator] 按 receiver 分流
    // （NativeFn 名与分派键对齐为 "MapSet.prototype.Symbol.iterator"）
    let cont_p = container_proto(vm);
    let f_ms = vm.alloc_native_fn("MapSet.prototype.Symbol.iterator");
    let _ = vm.set_property(Value::Object(cont_p), &iter_mkey, Value::Object(f_ms));
    register_handler(
        registry,
        "MapSet.prototype",
        "Symbol.iterator",
        map_set_iter_handler,
    );

    // 内建迭代器对象的属性面（见 iter.rs `attach_iterator_surface`）
    register_handler(
        registry,
        "Iterator.prototype",
        "next",
        iterator_next_handler,
    );
    register_handler(
        registry,
        "Iterator.prototype",
        "Symbol.iterator",
        iterator_self_handler,
    );
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
    // call/apply 语义的目标函数 = 调用 this（`fn.call(...)` 的 fn；
    // BoundFunction 转发时 this 即被绑目标——普通函数调用形态的
    // this 对 call/apply 无意义，不会到达本 handler）
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
/// 存自有属性；调用经 [`bound_fn_invoke`] 转发）。pub(crate) 供解释器的
/// CALL_METHOD bind 通用协议复用（NativeFn receiver 的 try_dispatch 回退
/// 会错误劫持 bind——见 interpreter）。
pub(crate) fn fn_proto_bind(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
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
    // 被调函数对象 = 绑定函数（普通调用 this 为 undefined，handler 经
    // pending_callee 取函数本体——_target/_this/_args 存于其 properties）
    let b = super::pending_callee();
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

/// `Number.prototype.X.call(num, ...)` 形态分派（真实包大量 `len.toString(16)`、
/// `(n).toFixed(2)` 等）。receiver 为原始值或 Number 包装对象。
pub(crate) fn num_method_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let full = super::pending_native_name();
    let name = full
        .rsplit("Number.prototype.")
        .next()
        .unwrap_or(&full)
        .to_owned();
    let this = super::current_receiver();
    let n = match this {
        Value::Number(n) => n,
        Value::Boolean(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        Value::Undefined => f64::NAN,
        Value::Null => 0.0,
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            // Number/Boolean 包装对象以 Ordinary 承载：读值槽，缺省字符串化
            Some(HeapObject::Ordinary { .. }) => vm
                .own_value(r.0 as usize, "[[NumberValue]]")
                .or_else(|| vm.own_value(r.0 as usize, "[[BooleanValue]]"))
                .and_then(|v| match v {
                    Value::Number(n) => Some(n),
                    Value::Boolean(b) => Some(if b { 1.0 } else { 0.0 }),
                    _ => None,
                })
                .unwrap_or_else(|| vm.format_value(this).parse().unwrap_or(f64::NAN)),
            _ => vm.format_value(this).parse().unwrap_or(f64::NAN),
        },
    };
    match name.as_str() {
        "toString" => {
            // toString([radix])：缺省 10；radix ∈ [2,36]；0/NaN 按 10
            let radix = args
                .first()
                .map(|v| crate::ops::to_number(*v))
                .unwrap_or(10.0);
            let out = if radix == 10.0 || radix.is_nan() {
                format_number_decimal(n)
            } else if (2.0..=36.0).contains(&radix) {
                let r = radix as u32;
                format_number_radix(n, r)
            } else {
                format_number_decimal(n)
            };
            Ok(Value::Object(vm.alloc_string(out)))
        }
        "valueOf" => Ok(Value::Number(n)),
        "toFixed" => {
            let digits = args
                .first()
                .map(|v| crate::ops::to_number(*v))
                .unwrap_or(0.0)
                .clamp(0.0, 100.0) as usize;
            Ok(Value::Object(vm.alloc_string(format!("{n:.digits$}"))))
        }
        "toExponential" => {
            let digits = args
                .first()
                .map(|v| crate::ops::to_number(*v))
                .unwrap_or(0.0);
            Ok(Value::Object(vm.alloc_string(format!(
                "{:.*e}",
                digits.clamp(0.0, 100.0) as usize,
                n
            ))))
        }
        "toPrecision" => {
            let p = args
                .first()
                .map(|v| crate::ops::to_number(*v))
                .unwrap_or(0.0);
            let out = if p <= 0.0 || p >= 21.0 {
                format_number_decimal(n)
            } else {
                let digits = p as usize;
                if n.abs() >= 10f64.powi(digits as i32 - 1) || n == 0.0 {
                    format!("{:.*}", digits - 1, n)
                } else {
                    format!("{:.*e}", digits - 1, n)
                }
            };
            Ok(Value::Object(vm.alloc_string(out)))
        }
        "toLocaleString" => Ok(Value::Object(vm.alloc_string(format_number_decimal(n)))),
        _ => {
            let msg = vm.alloc_string(format!("Number.prototype.{name} is not a function"));
            Err(VmError::Thrown(Value::Object(msg)))
        }
    }
}

/// 十进制数字字符串化（JS `String(n)` 形态：NaN/±Infinity 字面、
/// 整数不带尾零、否则最短十进制表示）。
fn format_number_decimal(n: f64) -> String {
    crate::ops::js_number_to_string(n)
}

/// 以给定进制格式化数字（2~36；整数位截断，负号保留）。
fn format_number_radix(n: f64, radix: u32) -> String {
    if n.is_nan() {
        return "NaN".to_owned();
    }
    if n == 0.0 {
        return "0".to_owned();
    }
    let neg = n < 0.0;
    let mut v = n.abs().trunc() as u64;
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while v > 0 {
        out.push(digits[(v % u64::from(radix)) as usize] as char);
        v /= u64::from(radix);
    }
    if out.is_empty() {
        out.push('0');
    }
    out.reverse();
    let mut s = out.into_iter().collect::<String>();
    if neg {
        s.insert(0, '-');
    }
    s
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

enum Either {
    Str(Vec<String>),
    Arr(Vec<Value>),
    None_,
}

/// `Array.from(source[, mapFn[, thisArg]])` / `Array.of(...items)`。
fn array_static_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let full = super::pending_native_name();
    let name = full.rsplit("Array.").next().unwrap_or(&full).to_owned();
    match name.as_str() {
        "from" => {
            let source = args.first().copied().unwrap_or(Value::Undefined);
            let mut items: Vec<Value> = match &source {
                Value::Undefined | Value::Null => {
                    let msg = vm.alloc_string(
                        "Array.from requires an array-like object - not null or undefined"
                            .to_owned(),
                    );
                    return Err(VmError::Thrown(Value::Object(msg)));
                }
                Value::Number(_) | Value::Boolean(_) => Vec::new(),
                Value::Object(r) => {
                    // 先克隆源数据结束 heap 借用，再做需要 &mut Vm 的装箱
                    let cloned = match vm.heap.get(r.0 as usize) {
                        Some(HeapObject::String(s)) => {
                            Either::Str(s.chars().map(|c| c.to_string()).collect::<Vec<_>>())
                        }
                        Some(HeapObject::Array { elements, .. }) => Either::Arr(elements.clone()),
                        _ => Either::None_,
                    };
                    match cloned {
                        Either::Str(chars) => chars
                            .into_iter()
                            .map(|c| Value::Object(vm.alloc_string(c)))
                            .collect(),
                        Either::Arr(elements) => elements,
                        Either::None_ => {
                            // 可迭代对象（Map/Set/四类内建迭代器/自定义 Symbol.iterator）
                            // 优先走迭代协议：它们在 Node 中是可迭代的，若按类数组
                            // （length + 数字下标）处理会静默得到空数组
                            // （`Array.from(new Map(...))` 曾为 `[]`）。
                            let iter_key = match vm.well_known_symbol("iterator") {
                                Value::Object(s) => crate::symbol::mangled_key(s),
                                _ => String::new(),
                            };
                            let iterable = !iter_key.is_empty()
                                && matches!(
                                    vm.get_property(source, &iter_key),
                                    Ok(Value::Object(_))
                                );
                            if iterable {
                                vm.collect_iter_values(source)?
                            } else {
                                // 类数组：length + 数字下标自属性
                                let len = vm
                                    .get_property(source, "length")
                                    .ok()
                                    .and_then(|v| match v {
                                        Value::Number(n) => Some(n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(0);
                                (0..len)
                                    .map(|i| {
                                        vm.get_property(source, &i.to_string())
                                            .unwrap_or(Value::Undefined)
                                    })
                                    .collect()
                            }
                        }
                    }
                }
            };
            // mapFn 变换（第 2 参数；第 3 参数 thisArg）
            if let Some(map_fn @ Value::Object(_)) = args.get(1).copied() {
                let this_arg = args.get(2).copied().unwrap_or(Value::Undefined);
                let mut mapped = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    let v =
                        vm.invoke_array_cb(map_fn, this_arg, &[*item, Value::Number(i as f64)])?;
                    mapped.push(v);
                }
                items = mapped;
            }
            Ok(Value::Object(vm.alloc_array(items)))
        }
        "of" => Ok(Value::Object(vm.alloc_array(args.to_vec()))),
        _ => Ok(Value::Undefined),
    }
}

/// `Array.prototype.X.call(arr, ...)` 形态分派（express 的
/// `$slice.call(arguments)` 等）：按方法实现核心语义，未实现返回 undefined。
fn array_method_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    use crate::value::Value as V;
    let full = super::pending_native_name();
    let name = full
        .rsplit("Array.prototype.")
        .next()
        .unwrap_or(&full)
        .to_owned();
    let this = super::current_receiver();
    let Value::Object(r) = this else {
        return Ok(Value::Undefined);
    };
    let elems = |vm: &Vm, r: aluka_core::ObjectRef| -> Vec<Value> {
        match vm.heap.get(r.0 as usize) {
            Some(HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        }
    };
    match name.as_str() {
        "slice" => {
            let parts = elems(vm, r);
            let len = parts.len() as i64;
            let norm = |v: Option<&Value>| -> usize {
                match v {
                    Some(V::Number(n)) => {
                        if *n < 0.0 {
                            ((len + *n as i64).max(0)) as usize
                        } else {
                            (*n as usize).min(parts.len())
                        }
                    }
                    _ => 0,
                }
            };
            let start = norm(args.first());
            let end = match args.get(1) {
                Some(V::Number(n)) => {
                    if *n < 0.0 {
                        ((len + *n as i64).max(0)) as usize
                    } else {
                        (*n as usize).min(parts.len())
                    }
                }
                None => parts.len(),
                _ => parts.len(),
            };
            let out = if start < end {
                parts[start..end].to_vec()
            } else {
                Vec::new()
            };
            Ok(Value::Object(vm.alloc_array(out)))
        }
        "concat" => {
            let mut out = elems(vm, r);
            for a in args {
                if let Value::Object(ar) = a
                    && matches!(vm.heap.get(ar.0 as usize), Some(HeapObject::Array { .. }))
                {
                    out.extend(elems(vm, *ar));
                } else {
                    out.push(*a);
                }
            }
            Ok(Value::Object(vm.alloc_array(out)))
        }
        "join" => {
            let sep = args
                .first()
                .map(|v| vm.format_value(*v))
                .unwrap_or_default();
            let text = elems(vm, r)
                .iter()
                .map(|v| vm.format_value(*v))
                .collect::<Vec<_>>()
                .join(&sep);
            Ok(Value::Object(vm.alloc_string(text)))
        }
        "toString" => {
            let text = elems(vm, r)
                .iter()
                .map(|v| vm.format_value(*v))
                .collect::<Vec<_>>()
                .join(",");
            Ok(Value::Object(vm.alloc_string(text)))
        }
        "push" => {
            let mut parts = elems(vm, r);
            for a in args {
                vm.gc_write_barrier(r, *a);
                parts.push(*a);
            }
            let len = parts.len();
            if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(r.0 as usize) {
                *elements = parts;
            }
            Ok(Value::Number(len as f64))
        }
        "pop" => {
            let mut parts = elems(vm, r);
            let out = parts.pop().unwrap_or(Value::Undefined);
            if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(r.0 as usize) {
                *elements = parts;
            }
            Ok(out)
        }
        "shift" => {
            // 删首元素并返回它、其余前移（空数组 → undefined）
            let mut parts = elems(vm, r);
            let out = if parts.is_empty() {
                Value::Undefined
            } else {
                parts.remove(0)
            };
            if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(r.0 as usize) {
                *elements = parts;
            }
            Ok(out)
        }
        "unshift" => {
            // 前插全部实参并返回新长度（实参顺序保持）
            let mut parts = elems(vm, r);
            for (i, a) in args.iter().enumerate() {
                vm.gc_write_barrier(r, *a);
                parts.insert(i, *a);
            }
            let len = parts.len();
            if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(r.0 as usize) {
                *elements = parts;
            }
            Ok(Value::Number(len as f64))
        }
        "indexOf" => {
            let needle = args.first().copied().unwrap_or(Value::Undefined);
            let pos = elems(vm, r)
                .iter()
                .position(|e| vm.values_same_zero(*e, needle));
            Ok(Value::Number(pos.map(|i| i as f64).unwrap_or(-1.0)))
        }
        "includes" => {
            let needle = args.first().copied().unwrap_or(Value::Undefined);
            let hit = elems(vm, r).iter().any(|e| vm.values_same_zero(*e, needle));
            Ok(Value::Boolean(hit))
        }
        "length" => Ok(Value::Number(elems(vm, r).len() as f64)),
        _ => Ok(Value::Undefined),
    }
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

// ===== Symbol.iterator handler 函数 =====

/// `String.prototype[Symbol.iterator]()`：返回字符串迭代器。
fn str_iter_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    match this {
        Value::Object(r) => Ok(vm.alloc_string_iterator(r)),
        _ => {
            let s =
                vm.alloc_string("String.prototype[Symbol.iterator] requires a string".to_owned());
            Err(VmError::Thrown(Value::Object(s)))
        }
    }
}

/// `Array.prototype[Symbol.iterator]()` → `values()`：返回数组元素值迭代器。
fn arr_iter_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    match this {
        Value::Object(r) => Ok(vm.alloc_array_iterator_kind(r, "values")),
        _ => Ok(Value::Undefined),
    }
}

/// `Map.prototype[Symbol.iterator]()` 与 `Set.prototype[Symbol.iterator]()`
/// 共用分派：经 Set 实例登记区分（Map/Set 共用 `HeapObject::Map` 变体，
/// 见 iter.rs）。Map 返回 entries 迭代器（产出 `[key, value]`），Set 返回
/// values 迭代器（产出 value）。
fn map_set_iter_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    match this {
        Value::Object(r) => {
            if vm.is_set_instance(this) {
                Ok(vm.alloc_set_iterator(r, "values"))
            } else if vm.is_map_instance(this) {
                Ok(vm.alloc_map_iterator(r, "entries"))
            } else {
                Ok(Value::Undefined)
            }
        }
        _ => Ok(Value::Undefined),
    }
}

/// 内建迭代器对象的 `next()` 属性面入口。
///
/// 与 `interpreter.rs` 中 CALL_METHOD 的按名硬编码分派等价，但走「属性读到的
/// NativeFn」路径——`typeof it.next === 'function'` 与 `it.next()` 因此都成立
/// （此前只有按名调用可用、属性读为 `undefined`）。
fn iterator_next_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = super::current_receiver();
    let Value::Object(r) = this else {
        return Ok(Value::Undefined);
    };
    if vm.is_array_iterator(this) {
        return vm.array_iterator_next(r);
    }
    if vm.is_string_iterator(this) {
        return vm.string_iterator_next(r);
    }
    if vm.is_map_iterator(this) {
        return vm.map_iterator_next(r);
    }
    if vm.is_set_iterator(this) {
        return vm.set_iterator_next(r);
    }
    Ok(Value::Undefined)
}

/// 迭代器对象的 `[Symbol.iterator]()`：返回自身（规范：迭代器对象自迭代）。
///
/// 没有这条属性时 `[...it]` / `Array.from(it)` / `Object.fromEntries(it)` 会
/// 读不到 `Symbol.iterator`，静默产生**空结果**（此前实测缺陷）。
fn iterator_self_handler(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(super::current_receiver())
}
