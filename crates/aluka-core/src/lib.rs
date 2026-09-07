//! 值系统、堆与隐藏类。
//!
//! 本 crate 是引擎的地基：它定义 [`Value`] 的表示、对象在堆上的布局、
//! 隐藏类（[`shape`]）与垃圾回收接口（[`gc`]）。其他 crate 只依赖这里
//! 导出的类型，不直接触碰堆的内部表示。
//!
//! # 设计约束（来自 Go 版实验结论）
//!
//! Go 版把 JS 值实现为 `interface{}`，因此撞上三个结构性上限：对象无法
//! bump 分配（`docs/adr/object-arena-rejected.md`）、槽位无法用无指针的
//! `u64`（`docs/adr/stage2-nanbox-slots-rejected.md`）、数值无法免装箱地
//! 存入接口。Rust 版把内存模型握在自己手里正是为了解开这三条，因此本
//! crate 的公开 API 必须保证：
//!
//! - [`Value`] 是 `Copy` 的机器字，数值不经堆；
//! - 堆对象的存活由本 crate 的 GC 判定，不依赖宿主语言的回收器；
//! - 引用的可达性显式经根集提供（见 [`gc::RootSet`]），不做隐式扫描。
//!
//! # 现状
//!
//! M0 阶段只固定 API 形状与不变量，实现体待 GC 选型定案后填充
//! （见 `aluka_r/docs/rust-reimplementation-devplan.md` M0 验收项）。

pub mod gc;
pub mod gc_protos;
pub mod object;
pub mod shape;
pub mod value;

pub use gc::{Heap, RootSet};
pub use object::ObjectRef;
pub use shape::{Shape, ShapeId, ShapeTable};
pub use value::{Value, ValueKind};
