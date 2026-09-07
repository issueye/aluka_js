//! [`Value`]：引擎的机器字值表示。
//!
//! JS 的七种原始类型加对象引用被压进一个 `Copy` 的机器字。M0 阶段先用
//! 显式 `enum` 固定语义边界；NaN-boxing（把 f64 与指针塞进同一个 `u64`）
//! 是 M0 的候选优化，需微基准对比后再切换——切换时本模块的公开 API
//! 保持不变，调用方不受影响。
//!
//! 之所以能考虑 NaN-boxing，是因为引擎自管 GC：Go 版的同款尝试因为
//! `u64` 里的指针对 Go GC 不可见而必然悬垂（见
//! `docs/adr/stage2-nanbox-slots-rejected.md`），Rust 版没有这个约束。

use crate::object::ObjectRef;

/// 值的类型标签，与 ECMAScript 的 `typeof` 谱系对齐（`Number` 覆盖
/// 整数与浮点，`Object` 覆盖数组/函数/Proxy 等一切堆对象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// `undefined`
    Undefined,
    /// `null`
    Null,
    /// `true` / `false`
    Boolean,
    /// IEEE-754 双精度；JS 不区分整数与浮点
    Number,
    /// 字符串（堆上的不可变序列，可能是 rope）
    String,
    /// 任意精度整数
    BigInt,
    /// 唯一符号
    Symbol,
    /// 一切堆对象：普通对象、数组、函数、Proxy……
    Object,
}

/// 引擎的值表示。
///
/// 数值内联在值里（不经堆），引用类型持有指向堆对象的 [`ObjectRef`]。
/// 该类型是 `Copy`：复制值不涉及分配，也不改变对象的存活状态——存活只由
/// [`crate::gc`] 的根集可达性决定。
///
/// **故意不派生 `PartialEq`**：Rust 的结构相等与 JS 相等语义处处冲突
/// （`NaN !== NaN`、`-0 === 0`、`0.1 + 0.2 !== 0.3`，对象比引用、原始值比内容）。
/// 派生一个"看着像相等"的实现会造出整类静默错判。判类型用 [`Value::kind`]，
/// 比大小走 ECMAScript 抽象操作。
#[derive(Debug, Clone, Copy)]
pub enum Value {
    /// `undefined`
    Undefined,
    /// `null`
    Null,
    /// 布尔
    Boolean(bool),
    /// 数值
    Number(f64),
    /// 堆对象（含字符串、BigInt、Symbol 与一般对象；具体品类由对象头区分）
    Object(ObjectRef),
}

impl Value {
    /// 返回值的类型标签。
    ///
    /// 堆对象需要读对象头才能区分字符串/BigInt/Symbol/一般对象，因此在
    /// 对象布局落地前一律报告 [`ValueKind::Object`]。
    #[must_use]
    pub fn kind(self) -> ValueKind {
        match self {
            Value::Undefined => ValueKind::Undefined,
            Value::Null => ValueKind::Null,
            Value::Boolean(_) => ValueKind::Boolean,
            Value::Number(_) => ValueKind::Number,
            Value::Object(_) => ValueKind::Object,
        }
    }

    /// 按 ECMAScript `ToBoolean` 求真值性。
    ///
    /// 注意 `NaN` 与 `-0` 均为假值，这与 Rust 的 `f64` 直觉不同；堆对象
    /// 恒为真（`document.all` 的历史例外不予实现）。
    #[must_use]
    pub fn to_boolean(self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Boolean(b) => b,
            Value::Number(n) => n != 0.0 && !n.is_nan(),
            Value::Object(_) => true,
        }
    }

    /// 是否为 `null` 或 `undefined`（可选链与空值合并的判定）。
    #[must_use]
    pub fn is_nullish(self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }

    /// 是否为堆对象引用。
    #[must_use]
    pub fn is_object(self) -> bool {
        matches!(self, Value::Object(_))
    }

    /// 是否为数值（整数与浮点共用 `f64`，JS 不区分）。
    #[must_use]
    pub fn is_number(self) -> bool {
        matches!(self, Value::Number(_))
    }

    /// 是否为布尔。
    #[must_use]
    pub fn is_boolean(self) -> bool {
        matches!(self, Value::Boolean(_))
    }

    /// 是否为 `undefined`。
    #[must_use]
    pub fn is_undefined(self) -> bool {
        matches!(self, Value::Undefined)
    }

    /// 是否为 `null`。
    #[must_use]
    pub fn is_null(self) -> bool {
        matches!(self, Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_boolean_matches_ecma_falsy_set() {
        assert!(!Value::Undefined.to_boolean());
        assert!(!Value::Null.to_boolean());
        assert!(!Value::Boolean(false).to_boolean());
        assert!(!Value::Number(0.0).to_boolean());
        assert!(!Value::Number(-0.0).to_boolean());
        assert!(!Value::Number(f64::NAN).to_boolean());

        assert!(Value::Boolean(true).to_boolean());
        assert!(Value::Number(1.0).to_boolean());
        assert!(Value::Number(f64::INFINITY).to_boolean());
    }

    #[test]
    fn nullish_covers_only_null_and_undefined() {
        assert!(Value::Undefined.is_nullish());
        assert!(Value::Null.is_nullish());
        assert!(!Value::Boolean(false).is_nullish());
        assert!(!Value::Number(0.0).is_nullish());
    }

    #[test]
    fn kind_reports_primitive_tags() {
        assert_eq!(Value::Undefined.kind(), ValueKind::Undefined);
        assert_eq!(Value::Number(1.0).kind(), ValueKind::Number);
        assert_eq!(Value::Boolean(true).kind(), ValueKind::Boolean);
    }
}
