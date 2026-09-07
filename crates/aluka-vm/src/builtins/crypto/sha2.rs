//! SHA-2 家族（FIPS 180-4）：SHA-256 / SHA-384 / SHA-512 纯 Rust 实现。
//!
//! SHA-256 为 512 位分组 64 轮；SHA-384/512 共用 1024 位分组 80 轮压缩，
//! 仅初始向量与截断长度不同。

// ---------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------

/// SHA-256 初始状态。
const SHA256_IV: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// SHA-256 轮常量。
const K256: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// SHA-256 压缩：以 64 字节分组更新 256 位状态。
pub(crate) fn compress256(state: &mut [u32; 8], block: &[u8]) {
    debug_assert!(block.len() >= 64);
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let b = &block[i * 4..i * 4 + 4];
        *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut v = *state;
    for i in 0..64 {
        let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
        let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
        let t1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K256[i])
            .wrapping_add(w[i]);
        let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(maj);
        v[7] = v[6];
        v[6] = v[5];
        v[5] = v[4];
        v[4] = v[3].wrapping_add(t1);
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        v[0] = t1.wrapping_add(t2);
    }
    for (s, x) in state.iter_mut().zip(v) {
        *s = s.wrapping_add(x);
    }
}

/// SHA-256 初始状态。
pub(crate) fn initial_state256() -> [u32; 8] {
    SHA256_IV
}

/// SHA-256 摘要长度（字节）。
pub(crate) const SHA256_OUT: usize = 32;

// ---------------------------------------------------------------------------
// SHA-512 / SHA-384
// ---------------------------------------------------------------------------

/// SHA-512 初始状态。
const SHA512_IV: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

/// SHA-384 初始状态（SHA-512 IV 截自 SHA-384 常量）。
const SHA384_IV: [u64; 8] = [
    0xcbbb_9d5d_c105_9ed8,
    0x629a_292a_367c_d507,
    0x9159_015a_3070_dd17,
    0x152f_ecd8_f70e_5939,
    0x6733_2667_ffc0_0b31,
    0x8eb4_4a87_6858_1511,
    0xdb0c_2e0d_64f9_8fa7,
    0x47b5_481d_befa_4fa4,
];

/// SHA-512 轮常量。
const K512: [u64; 80] = [
    0x428a_2f98_d728_ae22,
    0x7137_4491_23ef_65cd,
    0xb5c0_fbcf_ec4d_3b2f,
    0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538,
    0x59f1_11f1_b605_d019,
    0x923f_82a4_af19_4f9b,
    0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242,
    0x1283_5b01_4570_6fbe,
    0x2431_85be_4ee4_b28c,
    0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f,
    0x80de_b1fe_3b16_96b1,
    0x9bdc_06a7_25c7_1235,
    0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2,
    0xefbe_4786_384f_25e3,
    0x0fc1_9dc6_8b8c_d5b5,
    0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275,
    0x4a74_84aa_6ea6_e483,
    0x5cb0_a9dc_bd41_fbd4,
    0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab,
    0xa831_c66d_2db4_3210,
    0xb003_27c8_98fb_213f,
    0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2,
    0xd5a7_9147_930a_a725,
    0x06ca_6351_e003_826f,
    0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc,
    0x2e1b_2138_5c26_c926,
    0x4d2c_6dfc_5ac4_2aed,
    0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de,
    0x766a_0abb_3c77_b2a8,
    0x81c2_c92e_47ed_aee6,
    0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364,
    0xa81a_664b_bc42_3001,
    0xc24b_8b70_d0f8_9791,
    0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218,
    0xd699_0624_5565_a910,
    0xf40e_3585_5771_202a,
    0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8,
    0x1e37_6c08_5141_ab53,
    0x2748_774c_df8e_eb99,
    0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63,
    0x4ed8_aa4a_e341_8acb,
    0x5b9c_ca4f_7763_e373,
    0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc,
    0x78a5_636f_4317_2f60,
    0x84c8_7814_a1f0_ab72,
    0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28,
    0xa450_6ceb_de82_bde9,
    0xbef9_a3f7_b2c6_7915,
    0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c,
    0xd186_b8c7_21c0_c207,
    0xeada_7dd6_cde0_eb1e,
    0xf57d_4f7f_ee6e_d178,
    0x06f0_67aa_7217_6fba,
    0x0a63_7dc5_a2c8_98a6,
    0x113f_9804_bef9_0dae,
    0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84,
    0x32ca_ab7b_40c7_2493,
    0x3c9e_be0a_15c9_bebc,
    0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6,
    0x597f_299c_fc65_7e2a,
    0x5fcb_6fab_3ad6_faec,
    0x6c44_198c_4a47_5817,
];

/// SHA-512 压缩：以 128 字节分组更新 512 位状态。
pub(crate) fn compress512(state: &mut [u64; 8], block: &[u8]) {
    debug_assert!(block.len() >= 128);
    let mut w = [0u64; 80];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let b = &block[i * 8..i * 8 + 8];
        *word = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut v = *state;
    for i in 0..80 {
        let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
        let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
        let t1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K512[i])
            .wrapping_add(w[i]);
        let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(maj);
        v[7] = v[6];
        v[6] = v[5];
        v[5] = v[4];
        v[4] = v[3].wrapping_add(t1);
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        v[0] = t1.wrapping_add(t2);
    }
    for (s, x) in state.iter_mut().zip(v) {
        *s = s.wrapping_add(x);
    }
}

/// SHA-512 初始状态。
pub(crate) fn initial_state512() -> [u64; 8] {
    SHA512_IV
}

/// SHA-384 初始状态。
pub(crate) fn initial_state384() -> [u64; 8] {
    SHA384_IV
}

/// SHA-512 摘要长度（字节）。
pub(crate) const SHA512_OUT: usize = 64;

/// SHA-384 摘要长度（字节）。
pub(crate) const SHA384_OUT: usize = 48;

/// SHA-512 系分组长度（字节）。
pub(crate) const SHA512_BLOCK: usize = 128;

#[cfg(test)]
mod tests {
    use crate::builtins::crypto::digest::{Algo, Engine, to_hex};

    /// FIPS 180-4 官方向量（B.1 单块 / B.2 双块）。
    #[test]
    fn fips180_256_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
        ];
        for (input, want) in cases {
            let mut e = Engine::new(Algo::Sha256);
            e.update(input);
            assert_eq!(to_hex(&e.finalize()), *want);
        }
    }

    /// FIPS 180-4 SHA-384 官方向量。
    #[test]
    fn fips180_384_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"abc",
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                b"abcdefghbcdefghicdefghijefghijkfghijklijklmnojklmnopklmnopqlmnopqrs",
                "e120da23bff2ccefc918d2c6adf13d783188330b5177fd4eee868c4553ee3f700a4d66c66cf2f7eb0143eb294c1a56bd",
            ),
        ];
        for (input, want) in cases {
            let mut e = Engine::new(Algo::Sha384);
            e.update(input);
            assert_eq!(to_hex(&e.finalize()), *want);
        }
    }

    /// FIPS 180-4 SHA-512 官方向量。
    #[test]
    fn fips180_512_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"abc",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            (
                b"abcdefghbcdefghicdefghijefghijkfghijklijklmnojklmnopklmnopqlmnopqrs",
                "accdcdf01900065663ccd32031f7372f27b1b7c3e22b8425e18fa3881a444112ee66caa83614ced26d2f4ce666968668ecdd466d1ac80d34a6ac35cd28dc8963",
            ),
        ];
        for (input, want) in cases {
            let mut e = Engine::new(Algo::Sha512);
            e.update(input);
            assert_eq!(to_hex(&e.finalize()), *want);
        }
    }

    /// SHA-256 百万 'a' 向量（FIPS 180-4 B.3 类比）。
    #[test]
    fn million_a_sha256() {
        let mut e = Engine::new(Algo::Sha256);
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            e.update(&chunk);
        }
        assert_eq!(
            to_hex(&e.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// 增量喂入与一次性计算完全一致（覆盖跨块缓冲边界）。
    #[test]
    fn incremental_matches_one_shot() {
        let data: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        for algo in [Algo::Sha256, Algo::Sha384, Algo::Sha512] {
            let mut one = Engine::new(algo);
            one.update(&data);
            let mut inc = Engine::new(algo);
            for chunk in data.chunks(37) {
                inc.update(chunk);
            }
            assert_eq!(one.finalize(), inc.finalize(), "{algo:?} 增量不一致");
        }
    }
}
