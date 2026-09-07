//! GC 原型评测区（T-BE-02）：两个候选回收器的可运行原型。
//!
//! 本模块是**评测沙盒**：两个原型共用同一套对象模型（[`ProtoObject`]，
//! 槽位引用走 [`aluka_core::Value`] 的 [`aluka_core::ObjectRef`] 句柄），
//! 以便公平对比；评测结论落地时按选型重写 [`crate::gc::Heap`] 的真实实现。
//!
//! | 原型 | 策略 | 模块 |
//! |---|---|---|
//! | A | 分代标记-清除（非移动；年轻代 bump/free-list + 记忆集等价卡表 + 年龄晋升） | [`generational`] |
//! | B | 手写强引用计数 + 备份计数（trial delete）循环回收 | [`refcount`] |
//!
//! 两条铁律继承自已否决的 Go 侧实验（`docs/adr/`）：
//! 1. 句柄 [`ObjectRef`] 是堆内下标而非裸指针——GC 自管可达性，回收器
//!    看得见每一个槽位里的引用；
//! 2. 堆对象绝不进「指针数组 + 一块内存」的裸 arena——存活对象会钉住
//!    整块内存（RSS 放大 22-71×），原型 A 的清除只在 free-list 槽位粒度进行。

pub mod generational;
pub mod refcount;

use crate::object::ObjectClass;
use crate::value::Value;

/// 原型共用的堆对象：品类 + 槽位 + 分代/计数辅助字段。
#[derive(Debug, Clone)]
pub(crate) struct ProtoObject {
    /// 对象品类（决定槽位语义；评测负载中作标签，落地时驱动内部方法分派）
    #[allow(dead_code)]
    pub class: ObjectClass,
    /// 属性/元素槽位；`Value::Object` 的槽位是回收器必须追踪的引用
    pub slots: Vec<Value>,
    /// 分代年龄（原型 A：minor 存活次数，达到阈值晋升老年代）
    pub age: u8,
    /// 强引用计数（原型 B 专用；原型 A 恒为 0）
    pub strong: u32,
    /// 循环回收的备份计数（原型 B 专用暂存）
    pub buffered: u32,
    /// 循环回收的三色标记（原型 B 专用；0=未标记，1=黑，2=白）
    pub color: u8,
}

impl ProtoObject {
    /// 分配一个带 `slot_count` 个 `undefined` 槽位的对象。
    pub(crate) fn new(class: ObjectClass, slot_count: usize) -> Self {
        Self {
            class,
            slots: vec![Value::Undefined; slot_count],
            age: 0,
            strong: 0,
            buffered: 0,
            color: 0,
        }
    }

    /// 是否老年代（原型 A：`age` 达到晋升阈值）。
    pub(crate) fn is_old(&self, promote_age: u8) -> bool {
        self.age >= promote_age
    }

    /// 该槽位值是否为堆引用（回收器需要追踪）。
    pub(crate) fn slot_ref(value: Value) -> Option<crate::object::ObjectRef> {
        match value {
            Value::Object(r) => Some(r),
            _ => None,
        }
    }
}
