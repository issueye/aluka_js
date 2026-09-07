//! MD5 消息摘要算法（RFC 1321）纯 Rust 实现。
//!
//! Merkle–Damgård 结构：512 位分组、小端序长度、64 轮压缩。
//! 输出 128 位（16 字节）摘要。

/// MD5 压缩函数内部状态（A/B/C/D 四个 32 位字）。
const MD5_IV: [u32; 4] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];

/// 每轮循环左移位数（RFC 1321 表述）。
const SHIFT: [[u32; 4]; 4] = [
    [7, 12, 17, 22],
    [5, 9, 14, 20],
    [4, 11, 16, 23],
    [6, 10, 15, 21],
];

/// 轮常量 `K[i] = floor(abs(sin(i+1)) * 2^32)`。
const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// MD5 压缩：以 64 字节分组更新 128 位状态。
pub(crate) fn compress(state: &mut [u32; 4], block: &[u8]) {
    debug_assert!(block.len() >= 64);
    let mut m = [0u32; 16];
    for (i, word) in m.iter_mut().enumerate() {
        let b = &block[i * 4..i * 4 + 4];
        *word = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let tmp = d;
        d = c;
        c = b;
        let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
        b = b.wrapping_add(sum.rotate_left(SHIFT[i / 16][i % 4]));
        a = tmp;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

/// MD5 初始状态。
pub(crate) fn initial_state() -> [u32; 4] {
    MD5_IV
}

/// MD5 摘要长度（字节）。
pub(crate) const OUTPUT_LEN: usize = 16;

#[cfg(test)]
mod tests {
    use crate::builtins::crypto::digest::Engine;

    /// RFC 1321 §A.5 官方测试向量。
    #[test]
    fn rfc1321_vectors() {
        let cases: &[(&str, &str)] = &[
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            let mut e = Engine::new(crate::builtins::crypto::digest::Algo::Md5);
            e.update(input.as_bytes());
            assert_eq!(
                crate::builtins::crypto::digest::to_hex(&e.finalize()),
                *want
            );
        }
    }

    /// 增量喂入与一次性计算完全一致（Merkle–Damgård 必然成立，作回归锚）。
    #[test]
    fn incremental_matches_one_shot() {
        let data = vec![0x61u8; 1000];
        let mut one = Engine::new(crate::builtins::crypto::digest::Algo::Md5);
        one.update(&data);
        let mut inc = Engine::new(crate::builtins::crypto::digest::Algo::Md5);
        for chunk in data.chunks(7) {
            inc.update(chunk);
        }
        assert_eq!(one.finalize(), inc.finalize());
    }
}
