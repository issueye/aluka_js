//! SHA-1 消息摘要算法（FIPS 180-4 / RFC 3174）纯 Rust 实现。
//!
//! 512 位分组、大端序长度、80 轮压缩，输出 160 位（20 字节）摘要。

/// SHA-1 初始状态（h0..h4）。
const SHA1_IV: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

/// SHA-1 压缩：以 64 字节分组更新 160 位状态。
pub(crate) fn compress(state: &mut [u32; 5], block: &[u8]) {
    debug_assert!(block.len() >= 64);
    let mut w = [0u32; 80];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let b = &block[i * 4..i * 4 + 4];
        *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = match i / 20 {
            0 => ((b & c) | (!b & d), 0x5A82_7999),
            1 => (b ^ c ^ d, 0x6ED9_EBA1),
            2 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let tmp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = tmp;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// SHA-1 初始状态。
pub(crate) fn initial_state() -> [u32; 5] {
    SHA1_IV
}

/// SHA-1 摘要长度（字节）。
pub(crate) const OUTPUT_LEN: usize = 20;

#[cfg(test)]
mod tests {
    use crate::builtins::crypto::digest::{Algo, Engine, to_hex};

    /// FIPS 180-4 / RFC 6234 官方测试向量。
    #[test]
    fn fips180_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            ),
            (b"a", "86f7e437faa5a7fce15d1ddcb9eaeaea377667b8"),
            (
                b"0123456701234567012345670123456701234567012345670123456701234567",
                "e0c094e867ef46c350ef54a7f59dd60bed92ae83",
            ),
            (b"", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        ];
        for (input, want) in cases {
            let mut e = Engine::new(Algo::Sha1);
            e.update(input);
            assert_eq!(to_hex(&e.finalize()), *want);
        }
    }

    /// RFC 6234 百万 'a' 向量。
    #[test]
    fn million_a_vector() {
        let mut e = Engine::new(Algo::Sha1);
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            e.update(&chunk);
        }
        assert_eq!(
            to_hex(&e.finalize()),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    /// 增量喂入与一次性计算完全一致。
    #[test]
    fn incremental_matches_one_shot() {
        let data: Vec<u8> = (0..255u8).cycle().take(4096).collect();
        let mut one = Engine::new(Algo::Sha1);
        one.update(&data);
        let mut inc = Engine::new(Algo::Sha1);
        for chunk in data.chunks(13) {
            inc.update(chunk);
        }
        assert_eq!(one.finalize(), inc.finalize());
    }
}
