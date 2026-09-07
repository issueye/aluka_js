//! 原型 B：手写强引用计数 + 备份计数（trial delete）循环回收。
//!
//! 每个对象带 `strong` 计数：赋值槽位/登记根时增计，覆盖/移除时减计，
//! 减到 0 立即递归释放（析构确定性好、暂停分散）。环无法被纯计数回收，
//! 周期性执行 [`RefCycleHeap::collect_cycles`] 做备份计数检测
//! （Bacon-Rajan 风格 mark-sweep：试删内部引用 → 备份计数 >0 的是外部
//! 持有、==0 的是环内垃圾）。
//!
//! 与宿主 `Rc` 的本质区别：计数与释放完全由本 crate 管理，释放时机、
//! 暂停与统计对引擎可见——「存活由本 crate 判定」的约束不受影响。

use super::ProtoObject;
use crate::object::{ObjectClass, ObjectRef};
use crate::value::Value;

/// 三色标记：未标记 / 黑（确认存活）/ 白（待回收）。
const COLOR_NONE: u8 = 0;
const COLOR_BLACK: u8 = 1;
const COLOR_WHITE: u8 = 2;

/// 回收统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// 累计分配对象数
    pub allocated: u64,
    /// 当前存活对象数
    pub live: u64,
    /// 引用计数即时释放次数（dec 到 0）
    pub ref_reclaims: u64,
    /// 循环回收轮次
    pub cycle_collections: u64,
    /// 循环回收释放的对象数
    pub cycle_reclaimed: u64,
}

/// 引用计数堆（原型 B）。
#[derive(Debug, Default)]
pub struct RefCycleHeap {
    slab: Vec<Option<Box<ProtoObject>>>,
    free: Vec<u32>,
    /// 显式根表：每个根持有一个强计数（仅记录堆引用；原始值根是 no-op）
    roots: Vec<Option<ObjectRef>>,
    stats: Stats,
}

impl RefCycleHeap {
    /// 创建空堆。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配对象。新对象**无主**（`strong = 0`）：调用方必须立即把它登记为根
    /// （[`RefCycleHeap::add_root`]）或写入某个存活对象的槽位，否则它会一直
    /// 存活到下一次 [`RefCycleHeap::collect_cycles`] 作为零计数垃圾回收。
    pub fn allocate(&mut self, class: ObjectClass, slot_count: usize) -> ObjectRef {
        self.stats.allocated += 1;
        self.stats.live += 1;
        let obj = Box::new(ProtoObject::new(class, slot_count));
        if let Some(idx) = self.free.pop() {
            self.slab[idx as usize] = Some(obj);
            return ObjectRef(idx);
        }
        self.slab.push(Some(obj));
        ObjectRef((self.slab.len() - 1) as u32)
    }

    /// 登记根（强计数 +1；原始值无堆引用，登记无害）。
    pub fn add_root(&mut self, value: Value) {
        self.inc(value);
        self.roots.push(ProtoObject::slot_ref(value));
    }

    /// 移除根（强计数 -1，可能触发即时释放）。
    pub fn remove_root(&mut self, value: Value) {
        let target = ProtoObject::slot_ref(value);
        if let Some(pos) = self.roots.iter().position(|r| *r == target) {
            self.roots.swap_remove(pos);
            if let Some(r) = target {
                self.dec(Value::Object(r));
            }
        }
    }

    /// 读对象槽位。
    #[must_use]
    pub fn get_slot(&self, r: ObjectRef, slot: usize) -> Value {
        self.slab
            .get(r.index())
            .and_then(|s| s.as_deref())
            .and_then(|o| o.slots.get(slot))
            .copied()
            .unwrap_or(Value::Undefined)
    }

    /// 写对象槽位：新引用增计、旧引用减计（RC 的写屏障）。
    pub fn set_slot(&mut self, r: ObjectRef, slot: usize, value: Value) {
        self.inc(value);
        let old = {
            let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.as_mut()) else {
                return;
            };
            if slot >= obj.slots.len() {
                self.dec(value);
                return;
            }
            std::mem::replace(&mut obj.slots[slot], value)
        };
        self.dec(old);
    }

    /// 周期性循环回收：备份计数检测并释放纯环垃圾。返回本轮释放的对象数。
    ///
    /// 标准 trial-delete（Bacon-Rajan mark-sweep）：
    /// 1. 全候选备份强计数（`buffered = strong`），再统一对内部引用做代数减
    ///    （先全量备份再减，避免逐对象交替带来的顺序敏感）；
    /// 2. 扫描：备份计数 >0 的是黑（外部持有，连同其引用子树传播黑），
    ///    ==0 的是白（纯环内垃圾）；
    /// 3. 收集全部白色对象（对白色子树减强计数后释放）。
    pub fn collect_cycles(&mut self) -> u64 {
        let before = self.stats.cycle_reclaimed;
        let candidates: Vec<ObjectRef> = self
            .slab
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| s.as_ref().map(|_| ObjectRef(idx as u32)))
            .collect();
        // pass 1：备份强计数、清色
        for r in &candidates {
            let strong = self
                .slab
                .get(r.index())
                .and_then(|s| s.as_deref())
                .map(|o| o.strong)
                .unwrap_or(0);
            if let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.as_mut()) {
                obj.buffered = strong;
                obj.color = COLOR_NONE;
            }
        }
        // pass 2：统一减去内部引用
        for r in &candidates {
            let children: Vec<ObjectRef> = self
                .slab
                .get(r.index())
                .and_then(|s| s.as_deref())
                .map(|o| {
                    o.slots
                        .iter()
                        .filter_map(|s| ProtoObject::slot_ref(*s))
                        .collect()
                })
                .unwrap_or_default();
            for child in children {
                if let Some(c) = self.slab.get_mut(child.index()).and_then(|s| s.as_mut()) {
                    c.buffered = c.buffered.saturating_sub(1);
                }
            }
        }
        // pass 3：扫描定色（备份计数 >0 → 黑并传播；==0 → 白）
        for r in &candidates {
            let buffered = self
                .slab
                .get(r.index())
                .and_then(|s| s.as_deref())
                .map(|o| o.buffered)
                .unwrap_or(0);
            if buffered > 0 {
                self.scan_black(*r);
            } else if let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.as_mut()) {
                if obj.color == COLOR_NONE {
                    obj.color = COLOR_WHITE;
                }
            }
        }
        // pass 4：收集白色（对白色子树减强计数后释放）
        let whites: Vec<ObjectRef> = candidates
            .into_iter()
            .filter(|r| {
                self.slab
                    .get(r.index())
                    .and_then(|s| s.as_deref())
                    .is_some_and(|o| o.color == COLOR_WHITE)
            })
            .collect();
        let mut stack = whites;
        while let Some(r) = stack.pop() {
            let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.take()) else {
                continue;
            };
            for slot in &obj.slots {
                if let Value::Object(t) = slot {
                    let is_white = self
                        .slab
                        .get(t.index())
                        .and_then(|s| s.as_deref())
                        .is_some_and(|c| c.color == COLOR_WHITE);
                    if let Some(child) = self.slab.get_mut(t.index()).and_then(|s| s.as_mut()) {
                        child.strong = child.strong.saturating_sub(1);
                    }
                    if is_white {
                        stack.push(*t);
                    }
                }
            }
            self.free.push(r.0);
            self.stats.cycle_reclaimed += 1;
            self.stats.live -= 1;
        }
        // 清理根表中已释放的引用
        self.roots
            .retain(|r| !r.is_some_and(|r| self.slab.get(r.index()).is_none_or(|s| s.is_none())));
        self.stats.cycle_collections += 1;
        self.stats.cycle_reclaimed - before
    }

    /// 当前统计快照。
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    // ---- 内部 ----

    /// 强计数 +1（原始值无害）。
    fn inc(&mut self, value: Value) {
        if let Value::Object(r) = value {
            if let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.as_mut()) {
                obj.strong += 1;
            }
        }
    }

    /// 强计数 -1；减到 0 即时递归释放（引用计数的析构路径）。
    fn dec(&mut self, value: Value) {
        if let Value::Object(r) = value {
            let will_die = self
                .slab
                .get(r.index())
                .and_then(|s| s.as_deref())
                .is_some_and(|o| o.strong == 1);
            if will_die {
                self.release(r);
            } else if let Some(obj) = self.slab.get_mut(r.index()).and_then(|s| s.as_mut()) {
                obj.strong -= 1;
            }
        }
    }

    /// 释放对象：取走槽位、对子引用递归减计（迭代栈防深树爆栈）。
    fn release(&mut self, root: ObjectRef) {
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            let Some(obj) = self.slab.get_mut(cur.index()).and_then(|s| s.take()) else {
                continue;
            };
            self.stats.ref_reclaims += 1;
            self.stats.live -= 1;
            for slot in &obj.slots {
                if let Value::Object(t) = slot {
                    let child_will_die = self
                        .slab
                        .get(t.index())
                        .and_then(|s| s.as_deref())
                        .is_some_and(|o| o.strong == 1);
                    if child_will_die {
                        stack.push(*t);
                    } else if let Some(child) =
                        self.slab.get_mut(t.index()).and_then(|s| s.as_mut())
                    {
                        child.strong -= 1;
                    }
                }
            }
            self.free.push(cur.0);
        }
        // 契约外的根表悬挂引用同步清理
        self.roots
            .retain(|r| !r.is_some_and(|r| self.slab.get(r.index()).is_none_or(|s| s.is_none())));
    }

    /// scan 阶段：备份计数 >0——仍被外部持有，标记黑并沿子树传播。
    fn scan_black(&mut self, r: ObjectRef) {
        let mut stack = vec![r];
        while let Some(cur) = stack.pop() {
            let Some(obj) = self.slab.get_mut(cur.index()).and_then(|s| s.as_mut()) else {
                continue;
            };
            if obj.color == COLOR_BLACK {
                continue;
            }
            obj.color = COLOR_BLACK;
            let children: Vec<ObjectRef> = obj
                .slots
                .iter()
                .filter_map(|s| ProtoObject::slot_ref(*s))
                .collect();
            for child in children {
                let not_black = self
                    .slab
                    .get(child.index())
                    .and_then(|s| s.as_deref())
                    .is_some_and(|c| c.color != COLOR_BLACK);
                if not_black {
                    stack.push(child);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_root_reclaims_immediately() {
        let mut heap = RefCycleHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 0);
        heap.add_root(Value::Object(a));
        assert_eq!(heap.stats().live, 1);
        heap.remove_root(Value::Object(a));
        assert_eq!(heap.stats().live, 0, "根移除后应立即释放");
        assert_eq!(heap.stats().ref_reclaims, 1);
    }

    #[test]
    fn child_chain_releases_bottom_up() {
        let mut heap = RefCycleHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 1);
        let b = heap.allocate(ObjectClass::Ordinary, 0);
        heap.add_root(Value::Object(a));
        heap.add_root(Value::Object(b));
        heap.set_slot(a, 0, Value::Object(b)); // a -> b（b 计数 2：根 + a 槽位）
        heap.remove_root(Value::Object(b)); // 只留 a 根持有 b
        assert_eq!(heap.stats().live, 2);
        heap.remove_root(Value::Object(a)); // a 释放连带 b
        assert_eq!(heap.stats().live, 0, "链式引用应级联释放");
    }

    #[test]
    fn cycle_garbage_is_collected_by_backup_counting() {
        let mut heap = RefCycleHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 1);
        let b = heap.allocate(ObjectClass::Ordinary, 1);
        heap.add_root(Value::Object(a));
        heap.add_root(Value::Object(b));
        heap.set_slot(a, 0, Value::Object(b)); // a -> b
        heap.set_slot(b, 0, Value::Object(a)); // b -> a（环）
        heap.remove_root(Value::Object(a)); // 断开 a 的外部引用
        heap.remove_root(Value::Object(b)); // 断开 b 的外部引用：环成垃圾
        assert_eq!(heap.stats().live, 2, "纯计数无法回收环，仍存活");
        let freed = heap.collect_cycles();
        assert_eq!(freed, 2, "备份计数应回收整个环");
        assert_eq!(heap.stats().live, 0);
    }

    #[test]
    fn externally_referenced_cycle_survives() {
        let mut heap = RefCycleHeap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 1);
        let b = heap.allocate(ObjectClass::Ordinary, 1);
        heap.add_root(Value::Object(a));
        heap.add_root(Value::Object(b));
        heap.set_slot(a, 0, Value::Object(b));
        heap.set_slot(b, 0, Value::Object(a)); // 根持续持有的环
        let freed = heap.collect_cycles();
        assert_eq!(freed, 0, "被根持有的环不得回收");
        assert_eq!(heap.stats().live, 2);
        assert!(matches!(heap.get_slot(a, 0), Value::Object(r) if r == b));
    }

    #[test]
    fn shared_child_is_not_double_freed() {
        let mut heap = RefCycleHeap::new();
        let shared = heap.allocate(ObjectClass::Ordinary, 0);
        let p1 = heap.allocate(ObjectClass::Ordinary, 1);
        let p2 = heap.allocate(ObjectClass::Ordinary, 1);
        heap.add_root(Value::Object(p1));
        heap.add_root(Value::Object(p2));
        heap.set_slot(p1, 0, Value::Object(shared));
        heap.set_slot(p2, 0, Value::Object(shared));
        // 释放 p1：shared 仍被 p2 引用（计数 2->1），不得释放
        heap.remove_root(Value::Object(p1));
        assert_eq!(heap.stats().live, 2, "shared 应存活");
        assert!(matches!(heap.get_slot(p2, 0), Value::Object(r) if r == shared));
        heap.remove_root(Value::Object(p2));
        assert_eq!(heap.stats().live, 0);
    }
}
