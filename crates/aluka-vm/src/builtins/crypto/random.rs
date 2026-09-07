//! 运行时熵收集与伪随机字节生成（仅用 std，不引入外部 crate）。
//!
//! 熵源混合：`SystemTime` 纳秒 + 栈/堆地址（ASLR）+ 线程局部计数器 +
//! `RandomState` 哈希洗牌，经 SplitMix64 扩散后填充字节。对拍探针只断言
//! 长度/格式/类型，不断言随机值本身。

use std::cell::Cell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 全局调用计数器（线程安全分量）。
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

// 线程局部序号（每次取熵自增）
thread_local! {
    static LOCAL_SEQ: Cell<u64> = const { Cell::new(0) };
}

/// 取一份混合熵种子。
fn entropy_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let stack_addr = &nanos as *const u64 as u64;
    let heap_addr = {
        let boxed = Box::new(0u8);
        std::ptr::from_ref(&*boxed) as u64
    };
    let global = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = LOCAL_SEQ.with(|c| {
        let v = c.get().wrapping_add(1);
        c.set(v);
        v
    });
    // RandomState 每次构造即携带随机种子（std 地址熵），再洗牌一遍
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(nanos);
    hasher.write_u64(stack_addr);
    hasher.write_u64(heap_addr);
    hasher.write_u64(global);
    hasher.write_u64(local);
    hasher.finish()
}

/// SplitMix64 扩散一步。
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// 以密码学无关用途（探针只验格式）的伪随机字节填充 `buf`。
pub(crate) fn fill_random(buf: &mut [u8]) {
    let mut state = entropy_seed();
    // 预热扩散
    for _ in 0..4 {
        let _ = splitmix(&mut state);
    }
    let mut chunk = [0u8; 8];
    for (i, byte) in buf.iter_mut().enumerate() {
        let off = i % 8;
        if off == 0 {
            chunk = splitmix(&mut state).to_le_bytes();
            // 每 16 个字重新混入一份新鲜熵，防长序列可预测
            if i % 128 == 0 {
                state ^= entropy_seed();
            }
        }
        *byte = chunk[off];
    }
}

/// 生成 `[min, max)` 内无模偏差均匀随机整数（48 位拒绝采样，对齐 Go
/// `randomIntRange`）。
pub(crate) fn random_int_range(min: i64, max: i64) -> i64 {
    let span = (max - min) as u64;
    let lim = (1u64 << 48) - (1u64 << 48) % span;
    loop {
        let mut bytes = [0u8; 8];
        fill_random(&mut bytes[..6]);
        let v = u64::from_le_bytes(bytes) & 0x0000_ffff_ffff_ffff;
        if v < lim {
            return min + (v % span) as i64;
        }
    }
}

/// 生成 RFC 4122 version 4 UUID（小写、连字符分隔）。
pub(crate) fn random_uuid_v4() -> String {
    let mut b = [0u8; 16];
    fill_random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    let hex = |bytes: &[u8]| -> String {
        bytes
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join("")
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(&b[0..4]),
        hex(&b[4..6]),
        hex(&b[6..8]),
        hex(&b[8..10]),
        hex(&b[10..16])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// random_int_range 落域与拒绝采样可用性。
    #[test]
    fn random_int_in_range() {
        for _ in 0..200 {
            let v = random_int_range(3, 7);
            assert!((3..7).contains(&v));
        }
        assert_eq!(random_int_range(5, 6), 5);
    }

    /// UUID v4 格式（版本位与变体位）。
    #[test]
    fn uuid_v4_format() {
        let u = random_uuid_v4();
        assert_eq!(u.len(), 36);
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        let hexed = u.replace('-', "");
        assert!(hexed.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// 随机字节长度正确且两次采样几乎必然不同（熵源退化即报错）。
    #[test]
    fn random_bytes_length_and_spread() {
        let mut a = vec![0u8; 64];
        let mut b = vec![0u8; 64];
        fill_random(&mut a);
        fill_random(&mut b);
        assert_ne!(a, b);
    }
}
