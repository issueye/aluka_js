//! 垃圾回收接口。
//!
//! 与 Go 版的关键差别：**存活判定由本 crate 负责**，不再委托宿主语言的
//! 回收器。Go 版把 JS 对象实现为 Go 结构体，因此存活等于"Go GC 还没回收"，
//! 连带失去了 bump 分配、对象搬移与 NaN-box 槽位三种可能（见
//! `docs/adr/object-arena-rejected.md`、`stage2-nanbox-slots-rejected.md`）。
//!
//! # 根集必须显式提供
//!
//! Rust 的调用栈上没有可供扫描的类型信息，所以 GC 不做隐式栈扫描：
//! VM 需要通过 [`RootSet`] 把当前可达的起点（全局对象、帧内局部、操作数
//! 栈、闭包捕获单元、模块表、FFI 借出的句柄）交给 GC。漏报根 = 悬垂，
//! 这是本模块最重要的不变量。
//!
//! # 现状
//!
//! M0 阶段只固定接口。具体策略（分代标记-清除 vs 引用计数 + 循环回收）
//! 由 M0 的两份原型基准定夺，见 `aluka_r/docs/rust-reimplementation-devplan.md`
//! 的 M0 验收项。

use crate::object::{ObjectClass, ObjectRef};
use crate::value::Value;

/// GC 的根集：一次回收的可达性起点。
///
/// VM 每次触发回收前重建（或增量维护）它。不在根集中、也不被根集可达
/// 对象引用的堆对象即为垃圾。
#[derive(Debug, Default)]
pub struct RootSet {
    roots: Vec<Value>,
}

impl RootSet {
    /// 创建空根集。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个根。原始值（数值、布尔等）登记无害但无意义。
    pub fn push(&mut self, value: Value) {
        self.roots.push(value);
    }

    /// 遍历已登记的根。
    pub fn iter(&self) -> impl Iterator<Item = Value> + '_ {
        self.roots.iter().copied()
    }

    /// 弹出最近登记的根（调用栈式根管理：函数返回时撤销其局部根）。
    pub fn pop(&mut self) -> Option<Value> {
        self.roots.pop()
    }

    /// 移除第一个与 `value` 指向相同堆对象的根（原始值根不参与匹配——
    /// 登记原始根本就无害无意义）。
    pub fn remove(&mut self, value: Value) {
        let Value::Object(target) = value else {
            return;
        };
        if let Some(pos) = self
            .roots
            .iter()
            .position(|v| matches!(v, Value::Object(r) if *r == target))
        {
            self.roots.remove(pos);
        }
    }

    /// 根的数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// 根集是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// 清空根集，供下一轮重建复用底层容量。
    pub fn clear(&mut self) {
        self.roots.clear();
    }
}

/// 一次回收的统计，供 `--monitor` 与调优使用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    /// 累计分配对象数
    pub allocated: u64,
    /// 上次回收后存活对象数
    pub live: u64,
    /// 已完成的回收次数
    pub collections: u64,
}

/// 对象堆：分配、根登记与回收的生命周期入口。
///
/// **公共 API 已冻结（T-BE-03 / A1-4，见 `aluka_r/AGENTS.md` 冻结声明）**：
/// 下述方法签名是 M3 落地真实回收器（ADR `docs/adr/0002-aluka-r-gc-selection.md`
/// 定案的分代标记-清除）时必须保持的调用面。当前为占位实现（只记账不回收），
/// 用于把 VM 与内置库的接口先跑通。
#[derive(Debug, Default)]
pub struct Heap {
    next_index: u32,
    /// 增量登记的持久根（`add_root` / `remove_root` 维护）
    persistent_roots: RootSet,
    stats: GcStats,
}

impl Heap {
    /// 创建空堆。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配一个对象并返回其句柄。
    ///
    /// `class` 决定对象头之后的载荷布局，`slot_count` 预留属性/元素槽位。
    /// 当前实现只递增句柄计数——它足以让上层跑通接口，但不会真正存储
    /// 对象，因此**尚不能解引用**；M3 由分代标记-清除的真实分配器接替。
    pub fn allocate(&mut self, class: ObjectClass, slot_count: usize) -> ObjectRef {
        let _ = (class, slot_count);
        let handle = ObjectRef(self.next_index);
        self.next_index = self.next_index.wrapping_add(1);
        self.stats.allocated += 1;
        self.stats.live += 1;
        handle
    }

    /// 登记一个持久根（跨回收周期有效，如全局对象、模块表）。
    ///
    /// 原始值登记无害但无意义。与 [`Heap::collect_garbage_with`] 的批量
    /// 根集（操作数栈、帧局部等每次重建的瞬时根）互补。
    pub fn add_root(&mut self, value: Value) {
        self.persistent_roots.push(value);
    }

    /// 撤销一个持久根。
    pub fn remove_root(&mut self, value: Value) {
        self.persistent_roots.remove(value);
    }

    /// 以累积的持久根为起点执行一次回收，返回回收后的统计。
    ///
    /// 占位实现只累加回收计数：真实实现（M3）在这里完成标记（沿根集
    /// 遍历对象图）与分代清除。
    pub fn collect_garbage(&mut self) -> GcStats {
        self.stats.collections += 1;
        self.stats
    }

    /// 以显式根集执行一次回收（VM 每帧重建瞬时根集的批量模式），
    /// 持久根自动并入。返回回收后的统计。
    pub fn collect_garbage_with(&mut self, transient: &RootSet) -> GcStats {
        let _ = transient;
        self.stats.collections += 1;
        self.stats
    }

    /// 当前统计快照。
    #[must_use]
    pub fn stats(&self) -> GcStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_set_records_and_clears() {
        let mut roots = RootSet::new();
        assert!(roots.is_empty());

        roots.push(Value::Number(1.0));
        roots.push(Value::Undefined);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots.iter().count(), 2);

        roots.clear();
        assert!(roots.is_empty());
    }

    #[test]
    fn allocate_hands_out_distinct_handles_and_counts() {
        let mut heap = Heap::new();
        let a = heap.allocate(ObjectClass::Ordinary, 0);
        let b = heap.allocate(ObjectClass::Array, 4);

        assert_ne!(a, b);
        assert_eq!(heap.stats().allocated, 2);
        assert_eq!(heap.stats().live, 2);
    }

    #[test]
    fn collect_garbage_uses_persistent_roots() {
        let mut heap = Heap::new();
        let obj = heap.allocate(ObjectClass::Ordinary, 0);
        heap.add_root(Value::Object(obj));
        assert_eq!(heap.stats().collections, 0);

        let stats = heap.collect_garbage();
        assert_eq!(stats.collections, 1);
        // 登记过的根撤销后仍可再次回收
        heap.remove_root(Value::Object(obj));
        assert_eq!(heap.collect_garbage().collections, 2);
    }

    #[test]
    fn collect_garbage_with_accepts_transient_root_set() {
        let mut heap = Heap::new();
        let obj = heap.allocate(ObjectClass::Ordinary, 0);
        let mut transient = RootSet::new();
        transient.push(Value::Object(obj));

        let stats = heap.collect_garbage_with(&transient);
        assert_eq!(stats.collections, 1);
    }

    #[test]
    fn root_removal_matches_by_handle() {
        let mut roots = RootSet::new();
        let a = ObjectRef(1);
        let b = ObjectRef(2);
        roots.push(Value::Object(a));
        roots.push(Value::Object(b));

        roots.remove(Value::Object(a));
        assert_eq!(roots.len(), 1);
        assert!(
            roots
                .iter()
                .any(|v| matches!(v, Value::Object(r2) if r2 == b))
        );

        // 原始值根不参与 remove 匹配
        roots.push(Value::Number(1.0));
        roots.remove(Value::Number(1.0));
        assert_eq!(roots.len(), 2);
    }
}
