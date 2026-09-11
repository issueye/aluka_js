//! Proxy 反射代理子系统（ES2015+，语义对齐 Node.js 22 LTS）。
//!
//! 堆形态为 [`HeapObject::Proxy`]（target + handler + 撤销位）；13 种内部方法
//! 拦截经 handler 上的同名 trap 属性动态调用派发。trap 未安装时回退为对
//! target 的同型普通操作（规范 [[Call]]/[[Construct]] 等的默认行为）。
//!
//! 已知降级（规范允许范围内尽量保持行为一致）：
//! - 本运行时 Ordinary 属性模型不存储 configurability/writability 位，
//!   故 trap 结果的[[Invariant]] 校验（如 get 不得隐藏不可配置属性）以
//!   「全可配置 + 全可写」平凡满足，不做显式阻断；
//! - `ownKeys` 产出的键序以 trap 返回序为准，不做 target 键序校验。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

impl Vm {
    /// 判断值是否为 Proxy 对象。
    #[must_use]
    pub fn is_proxy(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Proxy { .. }))
        )
    }

    /// 读取 Proxy 的 (target, handler, revoked)；非 Proxy 返回 `None`。
    pub(crate) fn proxy_parts(&self, r: ObjectRef) -> Option<(ObjectRef, ObjectRef, bool)> {
        match self.heap.get(r.0 as usize) {
            Some(HeapObject::Proxy {
                target,
                handler,
                revoked,
            }) => Some((*target, *handler, *revoked)),
            _ => None,
        }
    }

    /// 撤销 Proxy（`revoke()` 语义）：置位后全部操作抛 TypeError。
    pub(crate) fn revoke_proxy(&mut self, r: ObjectRef) {
        if let Some(HeapObject::Proxy { revoked, .. }) = self.heap.get_mut(r.0 as usize) {
            *revoked = true;
        }
    }

    /// 构造调用校验并分配 Proxy：`new Proxy(target, handler)`（可调用形态同）。
    ///
    /// target/handler 必须为对象（含函数），否则按规范抛 TypeError。
    pub(crate) fn construct_proxy(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let target = args.first().copied().unwrap_or(Value::Undefined);
        let handler = args.get(1).copied().unwrap_or(Value::Undefined);
        if !matches!(target.case(), ValueCase::Object(_)) || !matches!(handler.case(), ValueCase::Object(_)) {
            let msg = "Cannot create proxy with a non-object as target or handler";
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(msg, "TypeError"),
            )));
        }
        let (ValueCase::Object(t), ValueCase::Object(h)) = (target, handler) else {
            unreachable!("上方已校验均为对象");
        };
        Ok(Value::Object(self.alloc_proxy(t, h)))
    }

    /// 分配带 name 的 TypeError 实例并包装为 Thrown（Proxy 规范错误形态）。
    pub(crate) fn alloc_typed_error(&mut self, msg: &str, name: &str) -> ObjectRef {
        let err = self.alloc_error_instance(msg);
        let name_ref = self.alloc_string(name.to_owned());
        let _ = self.set_property(Value::Object(err), "name", Value::Object(name_ref));
        err
    }

    /// 调用 handler 上的 trap 方法。返回 `Ok(None)` 表示 trap 未安装
    /// （回退默认行为）；`revoked` 状态在此先行拒绝。
    fn call_trap(
        &mut self,
        r: ObjectRef,
        trap: &str,
        args: &[Value],
    ) -> Result<Option<Value>, VmError> {
        let (target, handler, revoked) = self.proxy_parts(r).ok_or(VmError::StackUnderflow)?;
        if revoked {
            let msg = format!("Cannot perform '{trap}' on a proxy that has been revoked");
            return Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(&msg, "TypeError"),
            )));
        }
        // handler 可能在 trap 执行中被动态置换：每次取即时快照
        let trap_fn = self.get_property(Value::Object(handler), trap)?;
        if matches!(trap_fn, Value::Undefined | Value::Null) {
            return Ok(None);
        }
        let mut full_args = vec![Value::Object(target)];
        full_args.extend_from_slice(args);
        let ret = self.invoke_callable(trap_fn, Value::Undefined, &full_args)?;
        Ok(Some(ret))
    }

    /// [[Get]]：`get` trap → `handler.get(target, key, receiver)`。
    pub(crate) fn proxy_get(
        &mut self,
        r: ObjectRef,
        key: &str,
        receiver: Value,
    ) -> Result<Value, VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        match self.call_trap(r, "get", &[key_val, receiver])? {
            Some(v) => Ok(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.get_property(Value::Object(target), key)
            }
        }
    }

    /// [[Set]]：`set` trap → `handler.set(target, key, value, receiver)`。
    /// trap 返回假值时按规范抛 TypeError（严格模式赋值语义）。
    pub(crate) fn proxy_set(
        &mut self,
        r: ObjectRef,
        key: &str,
        val: Value,
        receiver: Value,
    ) -> Result<(), VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        let args = [key_val, val, receiver];
        let res = match self.call_trap(r, "set", &args)? {
            Some(v) => self.truthy(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.set_property(Value::Object(target), key, val)?;
                true
            }
        };
        if res {
            Ok(())
        } else {
            let msg = format!("'set' on proxy: trap returned falsish for property '{key}'");
            Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(&msg, "TypeError"),
            )))
        }
    }

    /// [[HasProperty]]：`has` trap → `handler.has(target, key)`。
    pub(crate) fn proxy_has(&mut self, r: ObjectRef, key: &str) -> Result<bool, VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        match self.call_trap(r, "has", &[key_val])? {
            Some(v) => Ok(self.truthy(v)),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                Ok(self.has_property(Value::Object(target), key))
            }
        }
    }

    /// [[Delete]]：`deleteProperty` trap。trap 返回假值抛 TypeError
    /// （严格模式 delete 语义）。
    pub(crate) fn proxy_delete(&mut self, r: ObjectRef, key: &str) -> Result<(), VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        let res = match self.call_trap(r, "deleteProperty", &[key_val])? {
            Some(v) => self.truthy(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.delete_property(Value::Object(target), key);
                true
            }
        };
        if res {
            Ok(())
        } else {
            let msg =
                format!("'deleteProperty' on proxy: trap returned falsish for property '{key}'");
            Err(VmError::Thrown(Value::Object(
                self.alloc_typed_error(&msg, "TypeError"),
            )))
        }
    }

    /// [[Call]]：`apply` trap → `handler.apply(target, thisArg, argsArray)`。
    /// 未安装时以 target 的 this/args 转发调用。
    pub(crate) fn proxy_apply(
        &mut self,
        r: ObjectRef,
        this_val: Value,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let args_arr = Value::Object(self.alloc_array(args.to_vec()));
        let args3 = [this_val, args_arr];
        match self.call_trap(r, "apply", &args3)? {
            Some(v) => Ok(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.invoke_callable(Value::Object(target), this_val, args)
            }
        }
    }

    /// [[Construct]]：`construct` trap → `handler.construct(target, argsArray, newTarget)`。
    /// 未安装时对 target 转发构造。
    pub(crate) fn proxy_construct(
        &mut self,
        r: ObjectRef,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let args_arr = Value::Object(self.alloc_array(args.to_vec()));
        let args2 = [args_arr, Value::Object(r)];
        match self.call_trap(r, "construct", &args2)?.map(|v| v.case()) {
            // 规范校验：construct trap 必须返回对象
            Some(v @ ValueCase::Object(_)) => Ok(v),
            Some(_) => Err(VmError::Thrown(Value::Object(self.alloc_typed_error(
                "'construct' on proxy: trap returned non-object ('undefined')",
                "TypeError",
            )))),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.do_construct(Value::Object(target), args)
            }
        }
    }

    /// [[GetPrototypeOf]]：`getPrototypeOf` trap。未安装回退 target 原型。
    pub(crate) fn proxy_get_prototype_of(&mut self, r: ObjectRef) -> Result<Value, VmError> {
        match self.call_trap(r, "getPrototypeOf", &[])? {
            Some(v) => Ok(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                Ok(self
                    .get_prototype(Value::Object(target))
                    .map(Value::Object)
                    .unwrap_or(Value::Null))
            }
        }
    }

    /// [[SetPrototypeOf]]：`setPrototypeOf` trap。返回操作是否成功。
    pub(crate) fn proxy_set_prototype_of(
        &mut self,
        r: ObjectRef,
        proto: Value,
    ) -> Result<bool, VmError> {
        match self.call_trap(r, "setPrototypeOf", &[proto])? {
            Some(v) => Ok(self.truthy(v)),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                let p = match proto.case() {
                    ValueCase::Object(pr) => Some(pr),
                    _ => None,
                };
                self.set_prototype_of(Value::Object(target), p);
                Ok(true)
            }
        }
    }

    /// [[IsExtensible]]：`isExtensible` trap。未安装回退 target 可扩展性
    /// （本运行时对象恒可扩展）。
    pub(crate) fn proxy_is_extensible(&mut self, r: ObjectRef) -> Result<bool, VmError> {
        match self.call_trap(r, "isExtensible", &[])? {
            Some(v) => Ok(self.truthy(v)),
            None => Ok(true),
        }
    }

    /// [[PreventExtensions]]：`preventExtensions` trap。返回操作是否成功。
    pub(crate) fn proxy_prevent_extensions(&mut self, r: ObjectRef) -> Result<bool, VmError> {
        match self.call_trap(r, "preventExtensions", &[])? {
            Some(v) => Ok(self.truthy(v)),
            None => Ok(true),
        }
    }

    /// [[GetOwnProperty]]：`getOwnPropertyDescriptor` trap。
    /// 返回描述对象或 undefined。
    pub(crate) fn proxy_get_own_property_descriptor(
        &mut self,
        r: ObjectRef,
        key: &str,
    ) -> Result<Value, VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        match self.call_trap(r, "getOwnPropertyDescriptor", &[key_val])? {
            Some(v) => Ok(v),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.ordinary_property_descriptor(Value::Object(target), key)
            }
        }
    }

    /// [[DefineOwnProperty]]：`defineProperty` trap。返回操作是否成功。
    pub(crate) fn proxy_define_property(
        &mut self,
        r: ObjectRef,
        key: &str,
        desc: Value,
    ) -> Result<bool, VmError> {
        let key_val = Value::Object(self.alloc_string(key.to_owned()));
        match self.call_trap(r, "defineProperty", &[key_val, desc])? {
            Some(v) => Ok(self.truthy(v)),
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                self.ordinary_define_property(Value::Object(target), key, desc)?;
                Ok(true)
            }
        }
    }

    /// [[OwnPropertyKeys]]：`ownKeys` trap。未安装回退 target 自有键。
    pub(crate) fn proxy_own_keys(&mut self, r: ObjectRef) -> Result<Vec<String>, VmError> {
        match self.call_trap(r, "ownKeys", &[])? {
            Some(keys_val) => {
                let items = self.to_array_values(keys_val);
                Ok(items.iter().map(|v| self.to_property_key(*v)).collect())
            }
            None => {
                let (target, _, _) = self.proxy_parts(r).unwrap();
                Ok(self
                    .own_properties(Value::Object(target))
                    .into_iter()
                    .map(|(k, _)| k)
                    .collect())
            }
        }
    }

    /// 撤销代理对函数性的暴露：typeof proxy 依 target 可调用性返回
    /// "function"/"object"（[[Call]] 内部槽透传语义）。
    pub(crate) fn proxy_typeof(&self, r: ObjectRef) -> String {
        if let Some((target, _, _)) = self.proxy_parts(r) {
            if let Some(HeapObject::Closure { .. } | HeapObject::NativeFn { .. }) =
                self.heap.get(target.0 as usize)
            {
                return "function".to_owned();
            }
        }
        "object".to_owned()
    }
}
