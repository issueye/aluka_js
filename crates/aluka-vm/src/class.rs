//! ES6 类声明、构造器装配与静态/实例继承实现。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Upvalue, Value, ValueCase};

impl Vm {
    /// 执行 Op::MakeClass 指令装配类原型与构造器。
    pub fn exec_make_class(&mut self, class_idx: usize) -> Result<(), VmError> {
        let class_tpl = self
            .module_classes
            .get(class_idx)
            .cloned()
            .ok_or(VmError::LocalOutOfRange)?;

        let num_computed = class_tpl.computed_indices.len();
        let mut computed_keys = Vec::with_capacity(num_computed);
        for _ in 0..num_computed {
            let k_val = self.pop()?;
            computed_keys.push(self.to_property_key(k_val));
        }
        computed_keys.reverse();

        let super_ctor = if class_tpl.has_super {
            Some(self.pop()?)
        } else {
            None
        };

        // 1. 创建 prototype 原型对象
        let super_proto_ref = if let Some(s) = super_ctor {
            match self.get_property(s, "prototype").map(|v| v.case()).map(ValueCase::from) {
                Ok(ValueCase::Object(p)) => Some(p),
                _ => {
                    if let Some(p) = s.as_object() {
                        Some(p)
                    } else {
                        None
                    }
                }
            }
        } else {
            None
        };
        let proto_ref = self.alloc_ordinary_with_proto(super_proto_ref);

        // 2. 创建构造函数闭包
        let ctor_func_idx = class_tpl.constructor_index as usize;
        let ctor_tmpl = self
            .module_functions
            .get(ctor_func_idx)
            .cloned()
            .ok_or(VmError::LocalOutOfRange)?;
        let mut captured = Vec::with_capacity(ctor_tmpl.upvalues.len());
        for cap in &ctor_tmpl.upvalues {
            if cap.is_local {
                let slot = cap.index as usize;
                let uv = self
                    .open_upvalues
                    .entry(slot)
                    .or_insert_with(|| {
                        let val = self.locals.get(slot).copied().unwrap_or(Value::Undefined);
                        Upvalue(std::rc::Rc::new(std::cell::RefCell::new(val)))
                    })
                    .clone();
                captured.push(uv);
            } else {
                let inherited = self
                    .current_upvalues
                    .get(cap.index as usize)
                    .cloned()
                    .unwrap_or_else(|| {
                        Upvalue(std::rc::Rc::new(std::cell::RefCell::new(Value::Undefined)))
                    });
                captured.push(inherited);
            }
        }
        let ctor_ref = self.alloc_closure_with_upvalues(ctor_func_idx, captured);
        self.set_property(
            Value::Object(ctor_ref),
            "prototype",
            Value::Object(proto_ref),
        )?;
        self.set_property(
            Value::Object(proto_ref),
            "constructor",
            Value::Object(ctor_ref),
        )?;

        // 静态继承：ctor 的 proto 指向 super_ctor
        if let Some(s_val) = super_ctor {
            let actual_super_ctor = match self.get_property(s_val, "constructor").map(|v| v.case()).map(ValueCase::from) {
                Ok(ValueCase::Object(c)) => Some(c),
                _ => {
                    if let Some(c) = s_val.as_object() {
                        Some(c)
                    } else {
                        None
                    }
                }
            };
            if let Some(s_ref) = actual_super_ctor {
                if let Some(HeapObject::Closure { proto, .. }) =
                    self.heap.get_mut(ctor_ref.0 as usize)
                {
                    *proto = Some(s_ref);
                }
            }
        }

        // 3. 安装方法与访问器
        let mut computed_pos = 0;
        for (mi, m) in class_tpl.methods.iter().enumerate() {
            let m_func_idx = m.func_index as usize;
            let m_tmpl = self
                .module_functions
                .get(m_func_idx)
                .cloned()
                .ok_or(VmError::LocalOutOfRange)?;
            let mut m_captured = Vec::with_capacity(m_tmpl.upvalues.len());
            for cap in &m_tmpl.upvalues {
                if cap.is_local {
                    let slot = cap.index as usize;
                    let uv = self
                        .open_upvalues
                        .entry(slot)
                        .or_insert_with(|| {
                            let val = self.locals.get(slot).copied().unwrap_or(Value::Undefined);
                            Upvalue(std::rc::Rc::new(std::cell::RefCell::new(val)))
                        })
                        .clone();
                    m_captured.push(uv);
                } else {
                    let inherited = self
                        .current_upvalues
                        .get(cap.index as usize)
                        .cloned()
                        .unwrap_or_else(|| {
                            Upvalue(std::rc::Rc::new(std::cell::RefCell::new(Value::Undefined)))
                        });
                    m_captured.push(inherited);
                }
            }
            let m_ref = self.alloc_closure_with_upvalues(m_func_idx, m_captured);
            let name = if computed_pos < class_tpl.computed_indices.len()
                && class_tpl.computed_indices[computed_pos] as usize == mi
            {
                let n = computed_keys[computed_pos].clone();
                computed_pos += 1;
                n
            } else {
                m.name.clone()
            };

            let target = if m.is_static {
                Value::Object(ctor_ref)
            } else {
                Value::Object(proto_ref)
            };

            match m.kind {
                0 => {
                    // 普通方法
                    self.set_property(target, &name, Value::Object(m_ref))?;
                }
                1 => {
                    // Getter（存方法闭包值，保留 upvalue 捕获）
                    if let Some(t_ref) = target.as_object() {
                        if let Some(HeapObject::Ordinary {
                            getters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(t_ref.0 as usize)
                        {
                            getters.insert(name, Value::Object(m_ref));
                            *has_accessors = 1;
                        }
                    }
                }
                2 => {
                    // Setter
                    if let Some(t_ref) = target.as_object() {
                        if let Some(HeapObject::Ordinary {
                            setters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(t_ref.0 as usize)
                        {
                            setters.insert(name, Value::Object(m_ref));
                            *has_accessors = 1;
                        }
                    }
                }
                _ => {}
            }
        }
        self.stack.push(Value::Object(ctor_ref));
        Ok(())
    }
}
