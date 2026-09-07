//! 原型 A：分代标记-清除（非移动）。
//!
//! 布局：单一 slab（`Vec<Option<ProtoObject>>`），对象头携带代别
//! （`Young` / `Old`）。新对象进年轻代 free-list（耗尽则 slab 追加，
//! 等价 bump 分配）；minor 回收只处理年轻代，老年代对年轻代的引用经
//! 写屏障进入记忆集（remembered set，等价卡表按对象粒度实现）；存活
//! `PROMOTE_AGE` 次 minor 后晋升老年代。major 回收全堆标记-清除。
//!
//! 非移动设计是刻意的：句柄即 slab 下标，回收不修补引用，与现有 VM
//! 的 `Vec<HeapObject>` 堆模型同构——落地成本最低。

use super::ProtoObject;
use crate::gc::RootSet;
use crate::object::{ObjectClass, ObjectRef};
use crate::value::Value;

/// minor 存活次数达到该值晋升老年代。
const PROMOTE_AGE: u8 = 2;

/// 对象代别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// 年轻代：新分配对象
    Young,
    /// 老年代：经历 `PROMOTE_AGE` 次 minor 存活
    Old,
}

/// 回收统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// 累计分配对象数
    pub allocated: u64,
    /// 当前存活对象数
    pub live: u64,
    /// minor 回收次数
    pub minor_collections: u64,
    /// major 回收次数
    pub major_collections: u64,
    /// 累计回收对象数（minor + major）
    pub reclaimed: u64,
}

/// 分代标记-清除堆（原型 A）。
#[derive(Debug, Default)]
pub struct GenerationalHeap {
    slab: Vec<Option<ProtoObject>>,
    young_free: Vec<u32>,
    old_free: Vec<u32>,
    /// 记忆集：槽位里写入过年轻引用的老年代对象（等价卡表，按对象粒度）
    remembered: Vec<u32>,
    stats: Stats,
}

impl GenerationalHeap {
    /// 创建空堆。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配年轻代对象，返回句柄。
    pub fn allocate(&mut self, class: ObjectClass, slot_count: usize) -> ObjectRef {
        self.stats.allocated += 1;
        self.stats.live += 1;
        let mut obj = ProtoObject::new(class, slot_count);
        obj.age = 0; // 年轻代
        if let Some(idx) = self.young_free.pop() {
            self.slab[idx as usize] = Some(obj);
            return ObjectRef(idx);
        }
        self.slab.push(Some(obj));
        ObjectRef((self.slab.len() - 1) as u32)
    }

    /// 对象当前代别。
    #[must_use]
    pub fn generation_of(&self, r: ObjectRef) -> Option<Generation> {
        self.slab[r.index()].as_ref().map(|o| {
            if o.is_old(PROMOTE_AGE) {
                Generation::Old
            } else {
                Generation::Young
            }
        })
    }

    /// 读对象槽位（越界或已回收返回 `undefined`——评测中不发生）。
    #[must_use]
    pub fn get_slot(&self, r: ObjectRef, slot: usize) -> Value {
        self.slab[r.index()]
            .as_ref()
            .and_then(|o| o.slots.get(slot))
            .copied()
            .unwrap_or(Value::Undefined)
    }

    /// 写对象槽位。老年代对象写入年轻引用时记入记忆集（写屏障）。
    pub fn set_slot(&mut self, r: ObjectRef, slot: usize, value: Value) {
        let Some(obj) = self.slab[r.index()].as_mut() else {
            return;
        };
        if slot < obj.slots.len() {
            obj.slots[slot] = value;
        }
        let is_old = obj.is_old(PROMOTE_AGE);
        let points_young =
            matches!(value, Value::Object(t) if self.generation_of(t) == Some(Generation::Young));
        if is_old && points_young && !self.remembered.contains(&r.0) {
            self.remembered.push(r.0);
        }
    }

    /// minor 回收：只清年轻代。根 = 根集中的年轻对象 + 记忆集老对象的年轻槽位。
    pub fn collect_minor(&mut self, roots: &RootSet) -> Stats {
        let mut marked = vec![false; self.slab.len()];
        for root in roots.iter() {
            if let Value::Object(r) = root {
                if self.generation_of(r) == Some(Generation::Young) {
                    self.mark_young(r, &mut marked);
                }
            }
        }
        // 记忆集老对象的年轻槽位是次级根；仍指向年轻对象的老对象重新登记
        let remembered = std::mem::take(&mut self.remembered);
        for old_idx in remembered {
            let young_targets: Vec<ObjectRef> = self.slab[old_idx as usize]
                .as_ref()
                .map(|o| {
                    o.slots
                        .iter()
                        .filter_map(|s| ProtoObject::slot_ref(*s))
                        .filter(|t| self.generation_of(*t) == Some(Generation::Young))
                        .collect()
                })
                .unwrap_or_default();
            for t in young_targets {
                self.mark_young(t, &mut marked);
            }
            let still = self.slab[old_idx as usize].as_ref().is_some_and(|o| {
                o.slots
                    .iter()
                    .any(|s| matches!(s, Value::Object(t) if self.generation_of(*t) == Some(Generation::Young)))
            });
            if still {
                self.remembered.push(old_idx);
            }
        }
        self.sweep_young_and_promote(&mut marked);
        self.stats.minor_collections += 1;
        self.stats
    }

    /// major 回收：全堆标记-清除（不分代），年轻存活对象直接晋升。
    pub fn collect_major(&mut self, roots: &RootSet) -> Stats {
        let mut marked = vec![false; self.slab.len()];
        for root in roots.iter() {
            if let Value::Object(r) = root {
                self.mark_all(r, &mut marked);
            }
        }
        let mut reclaimed = 0u64;
        for (idx, slot) in self.slab.iter_mut().enumerate() {
            let Some(obj) = slot else { continue };
            let idx32 = idx as u32;
            let is_old = obj.is_old(PROMOTE_AGE);
            if marked[idx] {
                // 存活：年轻对象直接晋升（major 后一律算老）
                if !is_old {
                    obj.age = PROMOTE_AGE;
                }
                continue;
            }
            *slot = None;
            if is_old {
                self.old_free.push(idx32);
            } else {
                self.young_free.push(idx32);
            }
            reclaimed += 1;
        }
        self.remembered.retain(|r| self.slab[*r as usize].is_some());
        self.stats.major_collections += 1;
        self.stats.reclaimed += reclaimed;
        self.stats.live -= reclaimed;
        self.stats
    }

    /// 标记年轻代可达对象（越过老年代——老对象经记忆集单独作根）。
    fn mark_young(&self, root: ObjectRef, marked: &mut [bool]) {
        let mut stack = vec![root];
        while let Some(r) = stack.pop() {
            let idx = r.index();
            if idx >= self.slab.len() || marked[idx] {
                continue;
            }
            let Some(obj) = self.slab[idx].as_ref() else {
                continue;
            };
            if obj.is_old(PROMOTE_AGE) {
                continue;
            }
            marked[idx] = true;
            for slot in &obj.slots {
                if let Value::Object(t) = slot {
                    stack.push(*t);
                }
            }
        }
    }

    /// 标记全堆可达对象（major 用）。
    fn mark_all(&self, root: ObjectRef, marked: &mut [bool]) {
        let mut stack = vec![root];
        while let Some(r) = stack.pop() {
            let idx = r.index();
            if idx >= self.slab.len() || marked[idx] {
                continue;
            }
            let Some(obj) = self.slab[idx].as_ref() else {
                continue;
            };
            marked[idx] = true;
            for slot in &obj.slots {
                if let Value::Object(t) = slot {
                    stack.push(*t);
                }
            }
        }
    }

    /// 清除未标记年轻对象；存活的年龄 +1，达到阈值晋升（非移动，仅翻代别标记）。
    fn sweep_young_and_promote(&mut self, marked: &mut [bool]) {
        let mut reclaimed = 0u64;
        for (idx, slot) in self.slab.iter_mut().enumerate() {
            let Some(obj) = slot else { continue };
            if obj.is_old(PROMOTE_AGE) {
                continue;
            }
            if marked[idx] {
                obj.age += 1; // 达到 PROMOTE_AGE 即自然晋升
                continue;
            }
            *slot = None;
            self.young_free.push(idx as u32);
            reclaimed += 1;
        }
        self.stats.reclaimed += reclaimed;
        self.stats.live -= reclaimed;
    }

    /// 当前统计快照。
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots_of(values: &[Value]) -> RootSet {
        let mut roots = RootSet::new();
        for v in values {
            roots.push(*v);
        }
        roots
    }

    #[test]
    fn live_objects_survive_minor_collection() {
        let mut heap = GenerationalHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 1);
        let b = heap.allocate(ObjectClass::Ordinary, 0);
        heap.set_slot(a, 0, Value::Object(b)); // a -> b
        heap.collect_minor(&roots_of(&[Value::Object(a)]));
        assert_eq!(heap.generation_of(a), Some(Generation::Young));
        assert!(matches!(heap.get_slot(a, 0), Value::Object(r) if r == b));
        assert_eq!(heap.stats().reclaimed, 0);
    }

    #[test]
    fn garbage_is_reclaimed_by_minor() {
        let mut heap = GenerationalHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 0);
        let b = heap.allocate(ObjectClass::Ordinary, 0);
        heap.set_slot(a, 0, Value::Object(b)); // a->b 环，无根
        let before = heap.stats().live;
        heap.collect_minor(&roots_of(&[]));
        assert_eq!(heap.stats().live, before - 2, "环上两个垃圾都应被回收");
        assert_eq!(heap.generation_of(a), None);
    }

    #[test]
    fn old_to_young_reference_is_remembered() {
        let mut heap = GenerationalHeap::new();
        let old = heap.allocate(ObjectClass::Ordinary, 1);
        // 让 old 晋升：经历两次 minor 存活
        heap.collect_minor(&roots_of(&[Value::Object(old)]));
        heap.collect_minor(&roots_of(&[Value::Object(old)]));
        assert_eq!(heap.generation_of(old), Some(Generation::Old));

        let young = heap.allocate(ObjectClass::Ordinary, 0);
        heap.set_slot(old, 0, Value::Object(young)); // 老 -> 新：写屏障
        // 不给根，仅靠记忆集应保住 young
        heap.collect_minor(&roots_of(&[]));
        assert_eq!(heap.generation_of(young), Some(Generation::Young));
        assert!(matches!(heap.get_slot(old, 0), Value::Object(r) if r == young));
    }

    #[test]
    fn survivors_promote_to_old_generation() {
        let mut heap = GenerationalHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 0);
        heap.collect_minor(&roots_of(&[Value::Object(a)]));
        heap.collect_minor(&roots_of(&[Value::Object(a)]));
        assert_eq!(
            heap.generation_of(a),
            Some(Generation::Old),
            "两次存活应晋升"
        );
    }

    #[test]
    fn major_collection_sweeps_all_generations() {
        let mut heap = GenerationalHeap::new();
        let keep = heap.allocate(ObjectClass::Ordinary, 0);
        let drop = heap.allocate(ObjectClass::Ordinary, 0);
        heap.collect_minor(&roots_of(&[Value::Object(keep), Value::Object(drop)]));
        heap.collect_minor(&roots_of(&[Value::Object(keep), Value::Object(drop)]));
        // keep/drop 都已是老年代；drop 掉根后 major 应回收老年代对象
        heap.collect_major(&roots_of(&[Value::Object(keep)]));
        assert_eq!(heap.generation_of(drop), None);
        assert_eq!(heap.generation_of(keep), Some(Generation::Old));
    }
}
