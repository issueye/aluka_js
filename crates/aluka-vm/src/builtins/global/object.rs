//! Object 构造器静态方法。

use crate::builtins::{current_receiver, pending_native_name};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

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
            if let Value::Object(r) = target {
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
            let p = match proto {
                Value::Object(pr) => Some(pr),
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
        "freeze" | "seal" => Ok(target),
        "isFrozen" | "isSealed" => Ok(Value::Boolean(false)),
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
            let list = args
                .first()
                .copied()
                .map(|v| vm.to_array_values(v))
                .unwrap_or_default();
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
