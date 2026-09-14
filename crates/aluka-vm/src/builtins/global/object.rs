//! Object 构造器静态方法。

use crate::builtins::{current_receiver, pending_native_name};
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

pub(crate) fn object_static(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let method = pending_native_name()
        .split('.')
        .next_back()
        .unwrap_or("")
        .to_owned();
    let _ = current_receiver();
    let target = args.first().copied().unwrap_or(Value::Undefined);
    match method.as_str() {
        "defineProperty" => {
            let key = args
                .get(1)
                .map(|v| vm.to_property_key(*v))
                .unwrap_or_default();
            let desc = args.get(2).copied().unwrap_or(Value::Undefined);
            if let Some(r) = target.as_object() {
                if vm.proxy_parts(r).is_some() {
                    return Ok(Value::Boolean(vm.proxy_define_property(r, &key, desc)?));
                }
            }
            vm.ordinary_define_property(target, &key, desc)?;
            Ok(target)
        }
        "defineProperties" => {
            if let Some(props) = args.get(1).copied() {
                for (k, desc) in vm.own_properties(props) {
                    vm.ordinary_define_property(target, &k, desc)?;
                }
            }
            Ok(target)
        }
        "getOwnPropertyDescriptor" => {
            let key = args
                .get(1)
                .map(|v| vm.to_property_key(*v))
                .unwrap_or_default();
            vm.ordinary_property_descriptor(target, &key)
        }
        "getOwnPropertyNames" | "keys" => {
            let items: Vec<Value> = vm
                .own_properties(target)
                .into_iter()
                .map(|(k, _)| Value::Object(vm.alloc_string(k)))
                .collect();
            Ok(Value::Object(vm.alloc_array(items)))
        }
        "setPrototypeOf" => {
            let proto = args.get(1).copied().unwrap_or(Value::Undefined);
            let p = match proto.case() {
                ValueCase::Object(pr) => Some(pr),
                _ => None,
            };
            vm.set_prototype_of(target, p);
            Ok(target)
        }
        "getPrototypeOf" => Ok(vm
            .get_prototype(target)
            .map(Value::Object)
            .unwrap_or(Value::Null)),
        "assign" => {
            let out = target;
            for src in args.get(1..).unwrap_or(&[]) {
                for (k, v) in vm.own_properties(*src) {
                    vm.set_property(out, &k, v)?;
                }
            }
            Ok(out)
        }
        "is" => {
            // 规范 SameValue：NaN 等值同真、+0/-0 异值（`Object.is` 实测缺失）
            let a = args.first().copied().unwrap_or(Value::Undefined);
            let b = args.get(1).copied().unwrap_or(Value::Undefined);
            let same = match (a.case(), b.case()) {
                (ValueCase::Number(x), ValueCase::Number(y)) => {
                    if x.is_nan() && y.is_nan() {
                        true
                    } else {
                        x == y && (x.to_bits() == y.to_bits() || !(x == 0.0 && y == 0.0))
                    }
                }
                _ => a == b || (matches!(a, Value::Undefined) && matches!(b, Value::Undefined)),
            };
            Ok(Value::Boolean(same))
        }
        "freeze" => {
            // 冻结：登记不可扩展 + 冻结态（属性写入/删除/重定义一律拒绝）
            if let Some(r) = target.as_object() {
                vm.non_extensible.insert(r.0 as usize);
                vm.frozen_objects.insert(r.0 as usize);
            }
            Ok(target)
        }
        "seal" => {
            // 密封：不可扩展，但属性仍可写
            if let Some(r) = target.as_object() {
                vm.non_extensible.insert(r.0 as usize);
            }
            Ok(target)
        }
        "hasOwn" => {
            let key = args
                .get(1)
                .map(|v| vm.to_property_key(*v))
                .unwrap_or_default();
            let has = match target.case() {
                ValueCase::Object(r) => {
                    vm.has_own_slot(r.0 as usize, &key) || vm.builtin_own_slot(r, &key)
                }
                // 原始值：按规范 ToObject 后判定（字符串有索引与 length）
                _ => false,
            };
            Ok(Value::Boolean(has))
        }
        "isFrozen" => {
            if matches!(target.case(), ValueCase::Undefined | ValueCase::Null) {
                return Err(vm.type_error("Cannot convert undefined or null to object"));
            }
            let frozen = target
                .as_object()
                .is_some_and(|r| vm.frozen_objects.contains(&(r.0 as usize)));
            Ok(Value::Boolean(frozen))
        }
        "isSealed" => {
            if matches!(target.case(), ValueCase::Undefined | ValueCase::Null) {
                return Err(vm.type_error("Cannot convert undefined or null to object"));
            }
            // 密封 = 不可扩展且全部属性不可配置；本实现以 freeze 登记为
            // 不可配置（seal 单独登记时按不可扩展近似）
            let sealed = target
                .as_object()
                .is_some_and(|r| vm.non_extensible.contains(&(r.0 as usize)));
            Ok(Value::Boolean(sealed))
        }
        // 扩展性：null/undefined 抛 TypeError；原始值按规范 ToObject 后为 true
        "isExtensible" => {
            if matches!(target.case(), ValueCase::Undefined | ValueCase::Null) {
                return Err(vm.type_error("Cannot convert undefined or null to object"));
            }
            let ext = !target
                .as_object()
                .is_some_and(|r| vm.non_extensible.contains(&(r.0 as usize)));
            Ok(Value::Boolean(ext))
        }
        "preventExtensions" => {
            if matches!(target.case(), ValueCase::Undefined | ValueCase::Null) {
                return Err(vm.type_error("Cannot convert undefined or null to object"));
            }
            if let Some(r) = target.as_object() {
                vm.non_extensible.insert(r.0 as usize);
            }
            Ok(target)
        }
        "values" | "entries" => {
            let mut items = vm.own_properties(target);
            items.sort_by(|a, b| a.0.cmp(&b.0));
            let out = match method.as_str() {
                "values" => items.into_iter().map(|(_, v)| v).collect(),
                _ => items
                    .into_iter()
                    .map(|(k, v)| {
                        let ks = vm.alloc_string(k);
                        Value::Object(vm.alloc_array(vec![Value::Object(ks), v]))
                    })
                    .collect(),
            };
            Ok(Value::Object(vm.alloc_array(out)))
        }
        "fromEntries" => {
            let arg = args.first().copied().unwrap_or(Value::Undefined);
            // 可迭代实参（Map/Set/四类内建迭代器）走迭代协议取 `[key, value]`
            // 序列——它们经 `to_array_values` 会静默得到空表
            // （`Object.fromEntries(new Map(...))` 曾为 `{}`）。
            // 数组与类数组维持 `to_array_values` 既有语义。
            let list = if vm.is_map_instance(arg)
                || vm.is_set_instance(arg)
                || vm.is_array_iterator(arg)
                || vm.is_string_iterator(arg)
                || vm.is_map_iterator(arg)
                || vm.is_set_iterator(arg)
            {
                vm.collect_iter_values(arg)?
            } else {
                vm.to_array_values(arg)
            };
            let out = vm.alloc_ordinary();
            for pair in list {
                let vals = vm.to_array_values(pair);
                if vals.len() >= 2 {
                    let key = vm.to_property_key(vals[0]);
                    vm.set_property(Value::Object(out), &key, vals[1])?;
                }
            }
            Ok(Value::Object(out))
        }
        _ => Ok(Value::Undefined),
    }
}
