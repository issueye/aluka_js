//! `Reflect` 全局对象与 `Proxy` 静态方法（Node.js 22 LTS 规范语义）。
//!
//! Reflect 提供 13 个规范静态方法，全部经 Proxy 感知的属性操作原语实现：
//! 目标为 Proxy 时自动走 trap 派发，目标为普通对象时走 Ordinary 语义。
//! `Proxy.revocable` / `Proxy.isProxy`（非规范但 Node 生态广泛使用）在此一并
//! 提供；`new Proxy(t, h)` 构造路径由解释器 `do_construct` 拦截。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// Reflect 全局对象（延迟物化单例；`resolve_global("Reflect")` 命中）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "reflect",
    build,
};

/// Reflect 单例（build 产出后经注册表模块表复用）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    Ok(materialize_with_registry(vm, registry))
}

/// 物化 Reflect 全局对象（未注册进内置模块表的全局路径用；幂等）。
///
/// 13 个规范方法 + 属性挂接；重复调用物化新表面（resolve_global 每 epoch 一次）。
pub fn materialize(vm: &mut Vm) -> ObjectRef {
    // 注册表临时取出避免双可变借用。窗口内模块单例不在 vm.builtin_registry
    // 根集合里（M6.1 根审计：压力模式下此窗口的分配会回收全部模块对象），
    // 以钉扎表补根，返回前弹出。
    let mut registry = std::mem::take(&mut vm.builtin_registry);
    let pins: Vec<u32> = registry.module_handles().map(|r| r.0).collect();
    vm.gc_pinned.extend_from_slice(&pins);
    let obj = materialize_with_registry(vm, &mut registry);
    vm.gc_pinned.truncate(vm.gc_pinned.len() - pins.len());
    vm.builtin_registry = registry;
    obj
}

fn materialize_with_registry(vm: &mut Vm, registry: &mut BuiltinRegistry) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    // 模块命名空间标记：try_dispatch 形态二（receiver = 模块对象）据此
    // 推导「Reflect.方法」分派键
    let ns = vm.alloc_string("Reflect".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    const METHODS: [&str; 13] = [
        "apply",
        "construct",
        "defineProperty",
        "deleteProperty",
        "get",
        "getOwnPropertyDescriptor",
        "getPrototypeOf",
        "has",
        "isExtensible",
        "ownKeys",
        "preventExtensions",
        "set",
        "setPrototypeOf",
    ];
    for m in METHODS {
        let name = concat_reflect(m);
        let f = vm.alloc_native_fn(&name);
        set_module_prop(vm, obj, m, Value::Object(f)).expect("Reflect 方法挂接失败");
        register_handler(registry, "Reflect", m, dispatch(m));
    }
    obj
}

/// 生成 `Reflect.<method>` 形式的原生函数名（try_dispatch 形态一键）。
fn concat_reflect(m: &str) -> String {
    format!("Reflect.{m}")
}

/// 按方法名选取处理器（处理器为 fn 指针，须静态分派）。
fn dispatch(m: &str) -> fn(&mut Vm, &[Value]) -> Result<Value, VmError> {
    match m {
        "apply" => reflect_apply,
        "construct" => reflect_construct,
        "defineProperty" => reflect_define_property,
        "deleteProperty" => reflect_delete_property,
        "get" => reflect_get,
        "getOwnPropertyDescriptor" => reflect_get_own_property_descriptor,
        "getPrototypeOf" => reflect_get_prototype_of,
        "has" => reflect_has,
        "isExtensible" => reflect_is_extensible,
        "ownKeys" => reflect_own_keys,
        "preventExtensions" => reflect_prevent_extensions,
        "set" => reflect_set,
        "setPrototypeOf" => reflect_set_prototype_of,
        _ => unreachable!("Reflect 方法名单封闭"),
    }
}

/// 在 Proxy 构造器单例上挂接静态方法与静态表面（`Vm::new` 末尾调用）。
pub fn setup_proxy_ctor(vm: &mut Vm, ctor: ObjectRef) {
    // 构造器单例注册为模块对象：try_dispatch 形态二据此反查「Proxy.方法」键
    vm.builtin_registry.register_module_object("Proxy", ctor);
    // Proxy.revocable(target, handler)：{ proxy, revoke }
    let revocable = vm.alloc_native_fn("Proxy.revocable");
    let _ = set_module_prop(vm, ctor, "revocable", Value::Object(revocable));
    register_handler(
        &mut vm.builtin_registry,
        "Proxy",
        "revocable",
        proxy_revocable,
    );
    // Proxy.isProxy(value)：非规范但 V8/Node 生态事实标准
    let is_proxy = vm.alloc_native_fn("Proxy.isProxy");
    let _ = set_module_prop(vm, ctor, "isProxy", Value::Object(is_proxy));
    register_handler(&mut vm.builtin_registry, "Proxy", "isProxy", proxy_is_proxy);
    // revoke() 闭包面：native fn 捕获 proxy 句柄（存自有属性表）
    register_handler(&mut vm.builtin_registry, "Proxy", "revoke", proxy_revoke);
}

/// `Proxy.revocable(target, handler)`：返回 `{ proxy, revoke }`。
fn proxy_revocable(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let proxy = vm.construct_proxy(args)?;
    if !matches!(proxy, Value::Object(_)) {
        unreachable!("construct_proxy 恒返回对象");
    }
    let revoke = vm.alloc_native_fn("Proxy.revoke");
    vm.set_native_fn_property(revoke, "_revokes", proxy);
    let pair = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(pair), "proxy", proxy);
    let _ = vm.set_property(Value::Object(pair), "revoke", Value::Object(revoke));
    Ok(Value::Object(pair))
}

/// `revoke()`：按捕获的 proxy 句柄撤销（每对 revocable 独立）。
///
/// 两条调用路径：`revoke()` 裸调用经 `invoke_callable` 特判（fn 对象即
/// 被调者）；`pair.revoke()` 方法调用经分派链（receiver 为 pair，从其
/// `revoke` 属性还原捕获的 fn 对象再读 `_revokes`）。
fn proxy_revoke(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = crate::builtins::current_receiver();
    if let Some(r) = receiver.as_object {
        if let Ok(Value::Object(fr)) = vm.get_property(Value::Object(r), "revoke") {
            if let Some(pr) = vm.get_native_fn_property(fr, "_revokes").as_object {
                vm.revoke_proxy(pr);
            }
        }
    }
    Ok(Value::Undefined)
}

/// `Proxy.isProxy(value)`：判断值是否为 Proxy 对象。
fn proxy_is_proxy(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let v = args.first().copied().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(vm.is_proxy(v)))
}

/// `Reflect.apply(target, thisArgument, argumentsList)`。
fn reflect_apply(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
    let list = args.get(2).copied().unwrap_or(Value::Undefined);
    let call_args = vm.to_array_values(list);
    vm.invoke_callable(target, this_arg, &call_args)
}

/// `Reflect.construct(target, argumentsList[, newTarget])`。
fn reflect_construct(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let list = args.get(1).copied().unwrap_or(Value::Undefined);
    let call_args = vm.to_array_values(list);
    vm.do_construct(target, &call_args)
}

/// `Reflect.defineProperty(target, propertyKey, attributes)`。
fn reflect_define_property(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let desc = args.get(2).copied().unwrap_or(Value::Undefined);
    // Proxy 目标经 trap 派发；普通对象走 Ordinary 定义
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            let ok = vm.proxy_define_property(r, &key, desc)?;
            return Ok(Value::Boolean(ok));
        }
    }
    vm.ordinary_define_property(target, &key, desc)?;
    Ok(Value::Boolean(true))
}

/// `Reflect.deleteProperty(target, propertyKey)`。
fn reflect_delete_property(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            vm.proxy_delete(r, &key)?;
            return Ok(Value::Boolean(true));
        }
    }
    vm.delete_property(target, &key);
    Ok(Value::Boolean(true))
}

/// `Reflect.get(target, propertyKey[, receiver])`。
fn reflect_get(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let receiver = args.get(2).copied().unwrap_or(target);
    // Proxy 目标经 get trap 派发（receiver 语义对齐规范第三参）
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return vm.proxy_get(r, &key, receiver);
        }
    }
    vm.get_property(target, &key)
}

/// `Reflect.getOwnPropertyDescriptor(target, propertyKey)`。
fn reflect_get_own_property_descriptor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return vm.proxy_get_own_property_descriptor(r, &key);
        }
    }
    vm.ordinary_property_descriptor(target, &key)
}

/// `Reflect.getPrototypeOf(target)`。
fn reflect_get_prototype_of(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return vm.proxy_get_prototype_of(r);
        }
    }
    Ok(vm
        .get_prototype(target)
        .map(Value::Object)
        .unwrap_or(Value::Null))
}

/// `Reflect.has(target, propertyKey)`。
fn reflect_has(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    Ok(Value::Boolean(vm.has_property(target, &key)))
}

/// `Reflect.isExtensible(target)`。
fn reflect_is_extensible(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return Ok(Value::Boolean(vm.proxy_is_extensible(r)?));
        }
    }
    Ok(Value::Boolean(matches!(target, Value::Object(_))))
}

/// `Reflect.ownKeys(target)`。
fn reflect_own_keys(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let keys: Vec<String> = if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            vm.proxy_own_keys(r)?
        } else {
            vm.own_properties(target)
                .into_iter()
                .map(|(k, _)| k)
                .collect()
        }
    } else {
        Vec::new()
    };
    let items: Vec<Value> = keys
        .into_iter()
        .map(|k| Value::Object(vm.alloc_string(k)))
        .collect();
    Ok(Value::Object(vm.alloc_array(items)))
}

/// `Reflect.preventExtensions(target)`。
fn reflect_prevent_extensions(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return Ok(Value::Boolean(vm.proxy_prevent_extensions(r)?));
        }
    }
    Ok(Value::Boolean(matches!(target, Value::Object(_))))
}

/// `Reflect.set(target, propertyKey, V[, receiver])`。
fn reflect_set(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let key = args
        .get(1)
        .map(|v| vm.to_property_key(*v))
        .unwrap_or_default();
    let val = args.get(2).copied().unwrap_or(Value::Undefined);
    let receiver = args.get(3).copied().unwrap_or(target);
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            vm.proxy_set(r, &key, val, receiver)?;
            return Ok(Value::Boolean(true));
        }
    }
    vm.set_property(target, &key, val)?;
    Ok(Value::Boolean(true))
}

/// `Reflect.setPrototypeOf(target, proto)`。
fn reflect_set_prototype_of(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let target = args.first().copied().unwrap_or(Value::Undefined);
    let proto = args.get(1).copied().unwrap_or(Value::Null);
    if let Some(r) = target.as_object {
        if vm.proxy_parts(r).is_some() {
            return Ok(Value::Boolean(vm.proxy_set_prototype_of(r, proto)?));
        }
    }
    let p = match proto {
        Value::Object(pr) => Some(pr),
        _ => None,
    };
    vm.set_prototype_of(target, p);
    Ok(Value::Boolean(true))
}
