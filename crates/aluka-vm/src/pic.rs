//! 解释器属性访问内联缓存（PIC，M6.3 切片一：读取路径）。
//!
//! **直接映射（direct-mapped）站点缓存**：`(当前函数 idx, pc)` 唯一确定一个
//! 属性访问点位，每点位缓存单一 `shape → 槽位` 绑定。命中时跳过
//! `shape_table.shape(id).lookup(key)` 的名字查表，直读槽位。
//!
//! # 命中前提（与慢路径语义逐条对齐，缺一即回退慢路径）
//!
//! 1. 站点键相等且缓存了 `(shape, slot)`；
//! 2. 对象为 Ordinary 且处于 **Shape 快速模式**（字典模式回慢路径）；
//! 3. 对象隐藏类 == 缓存 shape（ShapeId 不可变，等价即键→槽映射一致）；
//! 4. `deleted_gen == 0`（删除过属性的对象不得跳过删除语义）；
//! 5. `has_accessors == 0`（`defineProperty` 覆盖数据属性为访问器**不改
//!    shape**，数据槽仍在——必须以粘性标记拦截，见 `heap.rs` 字段注释）；
//! 6. 槽位下标在界内。
//!
//! # 写回资格（慢路径成功后一次性判定）
//!
//! 除上述 2/4/5 外，还要求键不在**魔法键表**内（流计算属性 / process 面 /
//! 知名符号等在 `get_property` 前置拦截的键——这些键的解析依赖对象隐含状态，
//! 不允许经 IC 直读），且 shape 键集不含 `_isStream`/`_isGlobalThis` 这类
//! 「存在即改变同名读取语义」的标记属性。

use crate::VmError;
use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::Vm;
use crate::jit_helpers::{from_vm_value, to_vm_value};
use crate::value::Value;

/// 直接映射槽位数（2 的幂）。同下标站点互挤时整体逐出——多态热点表现为
/// 持续 miss 回慢路径（语义仍正确），单态热点（绝大多数）零冲突。
const PIC_SLOTS: usize = 1 << 12;

/// 单点位缓存实体（16 字节，`site == 0` 表示空）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PropIcEntry {
    /// 站点键：`((func_idx + 1) << 32) | pc`（func_idx+1 ≥ 1 ⇒ 键恒非 0）
    pub site: u64,
    /// 缓存的对象隐藏类 id
    pub shape: u32,
    /// 缓存的槽位下标
    pub slot: u32,
}

/// 魔法键：`get_property` 前置拦截（流计算属性 / process 面）——这些键的
/// 结果依赖 STREAM_STORE / 进程单例状态，不可经 IC 槽位直读。
const MAGIC_KEYS: [&str; 13] = [
    "writableLength",
    "writableNeedDrain",
    "writableHighWaterMark",
    "readableLength",
    "readableHighWaterMark",
    "destroyed",
    "errored",
    "flowing",
    "env",
    "nextTick",
    "exit",
    "stderr",
    "stdout",
];

/// 全零 IC 表（`Vm::new` 初始化）。
#[must_use]
pub(crate) fn pic_table_new() -> Vec<PropIcEntry> {
    vec![
        PropIcEntry {
            site: 0,
            shape: 0,
            slot: 0,
        };
        PIC_SLOTS
    ]
}

/// 方法调用 IC 表槽数（站点数远少于属性读写，取 1/4 容量）。
const METHOD_IC_SLOTS: usize = 1 << 10;

/// 方法调用 IC 实体：绑定「receiver 隐藏类 → 直接原型上的方法槽位」。
///
/// 方法值**每次命中现读**（不缓存值本身）——原型同槽覆写新函数时无需失效
/// 即自动生效；其余原型变异（删除→字典化、defineProperty 访问器）经
/// `proto_shape` 比对与 `deleted_gen`/`has_accessors` 守卫拦截。
#[derive(Debug, Clone, Copy)]
pub(crate) struct MethodIcEntry {
    /// 站点键（0 = 空），与 [`PropIcEntry::site`] 同构
    pub site: u64,
    /// receiver 隐藏类 id（要求 receiver 自身**不含**该键——同 shape 键集一致）
    pub shape: u32,
    /// 直接原型的 ObjectRef（`set_prototype_of` 不改 shape，须显式比对）
    pub proto: u32,
    /// 原型隐藏类 id
    pub proto_shape: u32,
    /// 方法在原型槽位中的下标
    pub slot: u32,
}

/// 全零方法 IC 表（`Vm::new` 初始化）。
#[must_use]
pub(crate) fn method_ic_table_new() -> Vec<MethodIcEntry> {
    vec![
        MethodIcEntry {
            site: 0,
            shape: 0,
            proto: 0,
            proto_shape: 0,
            slot: 0,
        };
        METHOD_IC_SLOTS
    ]
}

impl Vm {
    /// 站点键（`current_func_idx` 由帧切换维护，`pc` 由主循环维护）。
    #[must_use]
    #[inline]
    pub(crate) fn pic_site(&self, pc: usize) -> u64 {
        let f = (self.current_func_idx.max(0) as u64).wrapping_add(1);
        (f << 32) | (pc as u64 & 0xFFFF_FFFF)
    }

    /// `Op::GetProp` / `Op::GetPropLocal` 的 IC 接入点：
    /// 命中直读槽位，否则走完整 `get_property` 并尝试写回。
    #[inline]
    pub(crate) fn get_property_ic(
        &mut self,
        obj: Value,
        key: &str,
        site: u64,
    ) -> Result<Value, VmError> {
        let idx = site as usize & (PIC_SLOTS - 1);
        let entry = self.prop_ic[idx];
        if entry.site == site
            && let Some(r) = obj.as_object()
            && let Some(HeapObject::Ordinary {
                props: OrdinaryProps::Shape { shape, slots, .. },
                deleted_gen,
                has_accessors,
                ..
            }) = self.heap.get(r.index())
            && shape.0 == entry.shape
            && *deleted_gen == 0
            && *has_accessors == 0
            && let Some(&b) = slots.get(entry.slot as usize)
        {
            self.pic_hits = self.pic_hits.wrapping_add(1);
            return Ok(to_vm_value(b));
        }
        let val = self.get_property(obj, key)?;
        self.pic_writeback(obj, key, site, idx);
        Ok(val)
    }

    /// 慢路径写回：仅缓存「可安全直读」的解析结果（资格见模块注释）。
    fn pic_writeback(&mut self, obj: Value, key: &str, site: u64, idx: usize) {
        if !self.pic_writeback_eligible(obj, key) {
            return;
        }
        if let Some(r) = obj.as_object()
            && let Some(HeapObject::Ordinary {
                props: OrdinaryProps::Shape { shape, .. },
                ..
            }) = self.heap.get(r.index())
        {
            let slot = self.shape_table.shape(*shape).and_then(|s| s.lookup(key));
            if let Some(slot) = slot
                && slot <= u32::MAX as usize
            {
                self.prop_ic[idx] = PropIcEntry {
                    site,
                    shape: shape.0,
                    slot: slot as u32,
                };
            }
        }
    }

    /// 写回资格：Ordinary + Shape 模式 + 无删除/访问器 + 键不在魔法表
    /// + shape 键集不含语义标记属性。
    fn pic_writeback_eligible(&self, obj: Value, key: &str) -> bool {
        if MAGIC_KEYS.contains(&key) {
            return false;
        }
        let Some(r) = obj.as_object() else {
            return false;
        };
        let Some(HeapObject::Ordinary {
            props: OrdinaryProps::Shape { shape, .. },
            deleted_gen,
            has_accessors,
            ..
        }) = self.heap.get(r.index())
        else {
            return false;
        };
        if *deleted_gen != 0 || *has_accessors != 0 {
            return false;
        }
        let Some(s) = self.shape_table.shape(*shape) else {
            return false;
        };
        !s.names().any(|n| n == "_isStream" || n == "_isGlobalThis")
    }

    /// `Op::SetProp` 家族的 IC 接入点：命中即槽位直写（覆盖既有槽），
    /// 否则走完整 `set_property` 并按资格写回（覆盖与追加路径皆可缓存——
    /// 追加完成后 shape 已含键，后续同 shape 写入即覆盖语义）。
    #[inline]
    pub(crate) fn set_property_ic(
        &mut self,
        obj: Value,
        key: &str,
        val: Value,
        site: u64,
    ) -> Result<(), VmError> {
        let idx = site as usize & (PIC_SLOTS - 1);
        let entry = self.prop_ic[idx];
        if entry.site == site
            && let Some(r) = obj.as_object()
            && let Some(HeapObject::Ordinary {
                props: OrdinaryProps::Shape { shape, slots, .. },
                deleted_gen,
                has_accessors,
                ..
            }) = self.heap.get_mut(r.index())
            && shape.0 == entry.shape
            && *deleted_gen == 0
            && *has_accessors == 0
            && let Some(b) = slots.get_mut(entry.slot as usize)
        {
            *b = from_vm_value(val);
            return Ok(());
        }
        self.set_property(obj, key, val)?;
        self.pic_writeback(obj, key, site, idx);
        Ok(())
    }

    /// `Op::CallMethod` 的方法解析 IC：绑定「receiver 隐藏类 → 直接原型上的
    /// 方法槽位」，命中时现读原型槽位值（语义与慢路径的原型链查找一致）。
    ///
    /// 仅缓存**深度 1 原型**解析（方法挂直接原型的主导形态）；receiver 自身
    /// 不得含同名自有键（同 shape 键集一致保证），多级原型链回退慢路径。
    #[inline]
    pub(crate) fn get_method_ic(
        &mut self,
        receiver: Value,
        key: &str,
        site: u64,
    ) -> Result<Value, VmError> {
        let idx = site as usize & (METHOD_IC_SLOTS - 1);
        let entry = self.method_ic[idx];
        if entry.site == site
            && let Some(r) = receiver.as_object()
            && let Some(HeapObject::Ordinary {
                props: OrdinaryProps::Shape { shape, .. },
                proto,
                deleted_gen,
                has_accessors,
                ..
            }) = self.heap.get(r.index())
            && shape.0 == entry.shape
            && *deleted_gen == 0
            && *has_accessors == 0
            && let Some(p) = proto
            && p.0 == entry.proto
            && let Some(HeapObject::Ordinary {
                props:
                    OrdinaryProps::Shape {
                        shape: p_shape,
                        slots,
                    },
                deleted_gen: p_deleted,
                has_accessors: p_accessors,
                ..
            }) = self.heap.get(p.0 as usize)
            && p_shape.0 == entry.proto_shape
            && *p_deleted == 0
            && *p_accessors == 0
            && let Some(&b) = slots.get(entry.slot as usize)
        {
            self.pic_hits = self.pic_hits.wrapping_add(1);
            return Ok(to_vm_value(b));
        }
        let val = self.get_property(receiver, key)?;
        self.method_ic_writeback(receiver, key, site, idx);
        Ok(val)
    }

    /// 方法 IC 写回：receiver 自身不含键且键落在**直接原型**槽位时才缓存。
    fn method_ic_writeback(&mut self, receiver: Value, key: &str, site: u64, idx: usize) {
        if MAGIC_KEYS.contains(&key) {
            return;
        }
        let (shape, proto_ref) = {
            let Some(r) = receiver.as_object() else {
                return;
            };
            let Some(HeapObject::Ordinary {
                props: OrdinaryProps::Shape { shape, .. },
                proto,
                deleted_gen,
                has_accessors,
                ..
            }) = self.heap.get(r.index())
            else {
                return;
            };
            if *deleted_gen != 0 || *has_accessors != 0 {
                return;
            }
            let Some(s) = self.shape_table.shape(*shape) else {
                return;
            };
            if s.names().any(|n| n == "_isStream" || n == "_isGlobalThis") {
                return;
            }
            (*shape, *proto)
        };
        let Some(p_ref) = proto_ref else {
            return;
        };
        let Some(HeapObject::Ordinary {
            props:
                OrdinaryProps::Shape {
                    shape: p_shape,
                    slots,
                },
            deleted_gen: p_deleted,
            has_accessors: p_accessors,
            ..
        }) = self.heap.get(p_ref.0 as usize)
        else {
            return;
        };
        if *p_deleted != 0 || *p_accessors != 0 {
            return;
        }
        let Some(p_shape_data) = self.shape_table.shape(*p_shape) else {
            return;
        };
        // receiver 自身不得含键（含键时解析走自有槽，不是原型绑定）
        if self
            .shape_table
            .shape(shape)
            .and_then(|s| s.lookup(key))
            .is_some()
        {
            return;
        }
        if let Some(slot) = p_shape_data.lookup(key)
            && slot <= u32::MAX as usize
            && slot < slots.len()
        {
            self.method_ic[idx] = MethodIcEntry {
                site,
                shape: shape.0,
                proto: p_ref.0,
                proto_shape: p_shape.0,
                slot: slot as u32,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Vm;

    /// 单态命中：同站点重复读取命中缓存，值与直读一致。
    #[test]
    fn monomorphic_hit_reads_correct_values() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "x", Value::Number(1.0));
        let _ = vm.set_property(Value::Object(o), "y", Value::Number(2.0));
        let site = vm.pic_site(7);
        let v1 = vm.get_property_ic(Value::Object(o), "x", site).unwrap();
        assert_eq!(v1.as_number(), Some(1.0));
        assert_eq!(vm.pic_hits, 0, "首次为冷 miss");
        let v2 = vm.get_property_ic(Value::Object(o), "x", site).unwrap();
        assert_eq!(v2.as_number(), Some(2.0 - 1.0));
        assert_eq!(vm.pic_hits, 1, "第二次命中 IC");
        let _ = vm.set_property(Value::Object(o), "x", Value::Number(9.0));
        let v3 = vm.get_property_ic(Value::Object(o), "x", site).unwrap();
        assert_eq!(v3.as_number(), Some(9.0), "写值后（同 shape 同槽）仍读新值");
    }

    /// 多态交替：两 shape 同站点互挤逐出，语义必须始终正确。
    #[test]
    fn polymorphic_alternation_stays_correct() {
        let mut vm = Vm::new(0);
        let a = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(a), "k", Value::Number(1.0));
        let b = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(b), "j", Value::Number(5.0));
        let _ = vm.set_property(Value::Object(b), "k", Value::Number(6.0));
        let site = vm.pic_site(3);
        for round in 0..4u32 {
            let va = vm.get_property_ic(Value::Object(a), "k", site).unwrap();
            let vb = vm.get_property_ic(Value::Object(b), "k", site).unwrap();
            assert_eq!(va.as_number(), Some(1.0), "round {round}");
            assert_eq!(vb.as_number(), Some(6.0), "round {round}");
        }
    }

    /// 访问器粘性：缓存后 `defineProperty` 覆盖为 getter（不改 shape），
    /// 读取必须触发 getter 而非直读数据槽。
    #[test]
    fn accessor_shadow_beats_cached_slot() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "v", Value::Number(1.0));
        let site = vm.pic_site(5);
        let warm = vm.get_property_ic(Value::Object(o), "v", site).unwrap();
        assert_eq!(warm.as_number(), Some(1.0));
        let _ = vm.get_property_ic(Value::Object(o), "v", site).unwrap();
        assert!(vm.pic_hits >= 1);
        // 注册 getter（has_accessors 置 1，shape 不变）——直接操作访问器表
        let getter = Value::Object(vm.alloc_native_fn("t.getter"));
        if let Some(HeapObject::Ordinary {
            getters,
            has_accessors,
            ..
        }) = vm.heap.get_mut(o.index())
        {
            getters.insert("v".to_owned(), getter);
            *has_accessors = 1;
        }
        // getter 表优先于数据槽：慢路径会真实触发访问器（未注册的原生
        // getter 被调用即抛错）——只要结果不是旧槽的 1.0，即证明 IC 守卫
        // 拦截成功、未跳过访问器语义
        let v = vm.get_property_ic(Value::Object(o), "v", site);
        assert!(v.is_err(), "应走慢路径触发访问器而非直读旧槽");
        assert_ne!(v.err().map(|e| format!("{e:?}")).unwrap(), "1.0");
    }

    /// 删除慢化：delete 后对象转字典，IC 守卫必须回慢路径。
    #[test]
    fn delete_switches_to_dict_and_ic_bails() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "d", Value::Number(3.0));
        let site = vm.pic_site(9);
        let _ = vm.get_property_ic(Value::Object(o), "d", site).unwrap();
        vm.delete_property(Value::Object(o), "d");
        let v = vm.get_property_ic(Value::Object(o), "d", site).unwrap();
        assert!(v.is_undefined(), "删除后读自有属性应为 undefined");
    }

    /// 原型回退：自有缺失经慢路径沿原型链取值（IC 不缓存原型解析）。
    #[test]
    fn proto_fallback_via_slow_path() {
        let mut vm = Vm::new(0);
        let proto_ref = vm.alloc_ordinary();
        let proto = Value::Object(proto_ref);
        let _ = vm.set_property(proto, "p", Value::Number(42.0));
        let child = vm.alloc_ordinary();
        vm.set_prototype_of(Value::Object(child), Some(proto_ref));
        let site = vm.pic_site(11);
        let v = vm.get_property_ic(Value::Object(child), "p", site).unwrap();
        assert_eq!(v.as_number(), Some(42.0));
    }
}

#[cfg(test)]
mod slice2_tests {
    use super::*;
    use crate::interpreter::Vm;

    /// 写 IC：同站点覆盖写命中槽位直写，读取面一致。
    #[test]
    fn write_ic_overwrite_keeps_visibility() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "w", Value::Number(1.0));
        let site = vm.pic_site(21);
        vm.set_property_ic(Value::Object(o), "w", Value::Number(2.0), site)
            .unwrap();
        vm.set_property_ic(Value::Object(o), "w", Value::Number(3.0), site)
            .unwrap();
        let v = vm.get_property(Value::Object(o), "w").unwrap();
        assert_eq!(v.as_number(), Some(3.0), "两次 IC 写后读面一致");
        // 追加路径：首次走慢路径建立 shape，二次起命中
        let o2 = vm.alloc_ordinary();
        vm.set_property_ic(Value::Object(o2), "w", Value::Number(7.0), site)
            .unwrap();
        vm.set_property_ic(Value::Object(o2), "w", Value::Number(8.0), site)
            .unwrap();
        let v2 = vm.get_property(Value::Object(o2), "w").unwrap();
        assert_eq!(v2.as_number(), Some(8.0), "追加后同 shape 写入走 IC");
    }

    /// 写 IC：setter 粘性拦截——缓存后 defineProperty 注册 setter，
    /// 写入必须触发 setter 语义（不得直写数据槽）。
    #[test]
    fn write_ic_setter_beats_cached_slot() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "s", Value::Number(1.0));
        let site = vm.pic_site(22);
        vm.set_property_ic(Value::Object(o), "s", Value::Number(2.0), site)
            .unwrap();
        // 注册 setter（has_accessors 置 1，shape 不变）
        let setter = Value::Object(vm.alloc_native_fn("t.setter"));
        if let Some(HeapObject::Ordinary {
            setters,
            has_accessors,
            ..
        }) = vm.heap.get_mut(o.index())
        {
            setters.insert("s".to_owned(), setter);
            *has_accessors = 1;
        }
        // 慢路径真实触发 setter（未注册原生 setter 被调用即抛错）——
        // 写入返回 Err 即证明走了访问器语义而非 IC 直写槽位
        let r = vm.set_property_ic(Value::Object(o), "s", Value::Number(9.0), site);
        assert!(r.is_err(), "setter 触发应产生错误而非静默直写");
        let v = vm.get_property(Value::Object(o), "s").unwrap();
        assert_ne!(v.as_number(), Some(9.0), "IC 写不得跳过 setter 语义");
    }

    /// 方法 IC：原型方法绑定命中 + 原型槽位覆写后现读新值。
    #[test]
    fn method_ic_binds_proto_and_reads_fresh() {
        let mut vm = Vm::new(0);
        let proto_ref = vm.alloc_ordinary();
        let proto = Value::Object(proto_ref);
        let m1 = Value::Object(vm.alloc_native_fn("m.one"));
        let _ = vm.set_property(proto, "run", m1);
        let child = vm.alloc_ordinary();
        vm.set_prototype_of(Value::Object(child), Some(proto_ref));
        let site = vm.pic_site(23);
        let v1 = vm.get_method_ic(Value::Object(child), "run", site).unwrap();
        assert_eq!(v1, m1, "首次冷解析绑定原型方法");
        let v2 = vm.get_method_ic(Value::Object(child), "run", site).unwrap();
        assert_eq!(v2, m1, "第二次命中 IC");
        // 原型同槽覆写：shape 不变，方法值现读必须拿到新函数
        let m2 = Value::Object(vm.alloc_native_fn("m.two"));
        let _ = vm.set_property(proto, "run", m2);
        let v3 = vm.get_method_ic(Value::Object(child), "run", site).unwrap();
        assert_eq!(v3, m2, "原型覆写后现读新方法值");
    }

    /// 方法 IC：自有属性遮蔽（own key）不得绑定原型槽；多态站点互挤正确。
    #[test]
    fn method_ic_own_shadow_and_polymorphism() {
        let mut vm = Vm::new(0);
        let proto_ref = vm.alloc_ordinary();
        let proto = Value::Object(proto_ref);
        let pm = Value::Object(vm.alloc_native_fn("p.m"));
        let _ = vm.set_property(proto, "go", pm);
        // a：无自有 go（走原型）
        let a = vm.alloc_ordinary();
        vm.set_prototype_of(Value::Object(a), Some(proto_ref));
        // b：自有 go 遮蔽（不走原型）
        let b = vm.alloc_ordinary();
        vm.set_prototype_of(Value::Object(b), Some(proto_ref));
        let own_m = Value::Object(vm.alloc_native_fn("b.own"));
        let _ = vm.set_property(Value::Object(b), "go", own_m);
        let site = vm.pic_site(24);
        let va = vm.get_method_ic(Value::Object(a), "go", site).unwrap();
        let vb = vm.get_method_ic(Value::Object(b), "go", site).unwrap();
        assert_eq!(va, pm, "a 经原型解析");
        assert_eq!(vb, own_m, "b 走自有槽（遮蔽原型）");
        // 互挤后再读：值仍各自正确
        let va2 = vm.get_method_ic(Value::Object(a), "go", site).unwrap();
        assert_eq!(va2, pm);
    }
}
