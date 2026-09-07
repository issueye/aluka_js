//! 堆对象与句柄。
//!
//! [`ObjectRef`] 是引用堆对象的**句柄**而非裸指针：它只是堆内的索引，
//! 因此 GC 搬移对象（copying/compacting）时无需修补散落各处的引用——
//! 这正是 Go 版做不到的一点（Go 的对象地址由 runtime 固定）。
//!
//! 句柄不代表所有权：对象的存活完全由 [`crate::gc`] 依根集可达性判定。
//! 持有一个 [`ObjectRef`] 不会让对象存活；把它放进根集才会。

/// 堆对象句柄。
///
/// 语义上是"堆内槽位下标"，`Copy` 且比较廉价。跨 GC 周期保持有效
/// （GC 搬移对象时更新的是堆内映射，不是句柄的值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectRef(pub u32);

impl ObjectRef {
    /// 句柄的数值形式，供堆内部索引使用。
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 堆对象的品类。
///
/// 决定对象头之后的载荷布局；`Ordinary` 之外都是 ECMAScript 意义上的
/// exotic object（有自定义的内部方法），实现时不能走普通属性快路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectClass {
    /// 普通对象：shape + 槽位
    Ordinary,
    /// 数组：packed 元素 + 洞位图 + `length` 语义
    Array,
    /// 函数/闭包
    Function,
    /// 字符串（含 rope）
    String,
    /// 任意精度整数
    BigInt,
    /// 唯一符号
    Symbol,
    /// Proxy：所有内部方法转发到 handler
    Proxy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ref_exposes_index() {
        assert_eq!(ObjectRef(7).index(), 7);
    }

    #[test]
    fn object_class_distinguishes_exotic_kinds() {
        assert_ne!(ObjectClass::Ordinary, ObjectClass::Array);
        assert_ne!(ObjectClass::Array, ObjectClass::Proxy);
    }
}
