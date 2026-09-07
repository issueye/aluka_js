//! JIT 值域：NaN-box u64（JSC 风格）。
//!
//! 机器码只认 `u64` 机器字。值域编码规则：
//! - **Number**：直接存 f64 原始比特（`to_bits`），**入盒时 NaN 一律规范化**到
//!   0x7FF8_0000_0000_0000（x86 算术 NaN 即此值）。tag 空间取 top16 == 0xFFF7，
//!   与 0x7FF8 永不碰撞，因此 f64 比特可无损通过（jitdiff 逐位相等的根基）；
//! - **非数值**：`0xFFF7_0000_0000_0000 | (tag)`，tag 在最低字节，ObjectRef
//!   占 8..=39 位。
//!
//! 本模块同时是 aluka-vm helper 与 JIT 代码生成共享的编解码唯一事实来源。

/// tag 空间前缀：top16 == 0xFFF7（NaN 空间内的一块，算术永不产出）。
pub const TAG_PREFIX: u64 = 0xFFF7_0000_0000_0000;
/// tag 判定掩码。
pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
/// 入盒规范化后的数字 NaN（x86 算术 NaN 的标准形态）。
pub const NAN_CANONICAL: u64 = 0x7FF8_0000_0000_0000;

/// 非数值 tag（最低字节）：`undefined`。
pub const TAG_UNDEFINED: u64 = 0;
/// 非数值 tag（最低字节）：`null`。
pub const TAG_NULL: u64 = 1;
/// 非数值 tag（最低字节）：`false`。
pub const TAG_FALSE: u64 = 2;
/// 非数值 tag（最低字节）：`true`。
pub const TAG_TRUE: u64 = 3;
/// 非数值 tag（最低字节）：对象引用（8..=39 位为 ObjectRef）。
pub const TAG_OBJECT: u64 = 4;

/// `undefined` 的盒表示。
pub const UNDEFINED: u64 = TAG_PREFIX | TAG_UNDEFINED;
/// `null` 的盒表示。
pub const NULL: u64 = TAG_PREFIX | TAG_NULL;
/// `false` 的盒表示。
pub const FALSE: u64 = TAG_PREFIX | TAG_FALSE;
/// `true` 的盒表示。
pub const TRUE: u64 = TAG_PREFIX | TAG_TRUE;

/// 判断盒是否为数值（非 tag 空间即数字）。
#[must_use]
pub const fn is_number(b: u64) -> bool {
    (b & TAG_MASK) != TAG_PREFIX
}

/// 判断盒是否为对象引用。
#[must_use]
pub const fn is_object(b: u64) -> bool {
    (b & TAG_MASK) == TAG_PREFIX && (b & 0xFF) == TAG_OBJECT
}

/// 数值入盒：f64 比特直存，NaN 规范化到 [`NAN_CANONICAL`]。
#[must_use]
pub fn box_number(n: f64) -> u64 {
    if n.is_nan() {
        NAN_CANONICAL
    } else {
        n.to_bits()
    }
}

/// 数值出盒（调用方须已确认 [`is_number`]）。
#[must_use]
pub fn unbox_number(b: u64) -> f64 {
    f64::from_bits(b)
}

/// 对象引用入盒。
#[must_use]
pub fn box_object(r: u32) -> u64 {
    TAG_PREFIX | TAG_OBJECT | ((r as u64) << 8)
}

/// 出盒取 ObjectRef（调用方须已确认 [`is_object`]）。
#[must_use]
pub fn unbox_object(b: u64) -> u32 {
    ((b >> 8) & 0xFFFF_FFFF) as u32
}

/// 真值性快速路径：数值域内（非零且非 NaN 为真；对齐 VM `to_boolean`）。
/// 非数值必须走 helper，本函数不负责。
#[must_use]
pub fn fast_truthy_number(b: u64) -> bool {
    let n = unbox_number(b);
    n != 0.0 && !n.is_nan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_roundtrip_bit_exact() {
        for n in [
            0.0,
            -0.0,
            1.0,
            -1.5,
            f64::MAX,
            f64::MIN_POSITIVE,
            std::f64::consts::PI,
        ] {
            let b = box_number(n);
            assert!(is_number(b));
            assert_eq!(
                unbox_number(b).to_bits(),
                n.to_bits(),
                "非 NaN 必须逐位往返"
            );
        }
    }

    #[test]
    fn nan_is_canonicalized_and_still_number() {
        let b = box_number(f64::NAN);
        assert!(is_number(b), "NaN 规范化后仍在数值域（不与 tag 空间碰撞）");
        assert_eq!(b, NAN_CANONICAL);
        assert!(unbox_number(b).is_nan());
        // 带 payload 的 NaN 也规范化
        let payload_nan = f64::from_bits(0x7FF0_0000_0000_0001);
        assert_eq!(box_number(payload_nan), NAN_CANONICAL);
    }

    #[test]
    fn tags_do_not_collide_with_number_space() {
        for v in [
            UNDEFINED,
            NULL,
            FALSE,
            TRUE,
            box_object(0),
            box_object(0xDEAD_BEEF),
        ] {
            assert!(!is_number(v), "tag 值不得落入数值域");
        }
    }

    #[test]
    fn object_ref_roundtrip() {
        for r in [0u32, 1, 0xDEAD_BEEF, u32::MAX] {
            let b = box_object(r);
            assert!(is_object(b));
            assert_eq!(unbox_object(b), r);
        }
    }
}
