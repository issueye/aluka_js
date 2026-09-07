//! HMAC 消息认证码（RFC 2104 / FIPS 198-1）纯 Rust 实现。
//!
//! `H(K XOR opad, H(K XOR ipad, text))`，块长取底层摘要分组长度
//! （md5/sha1/sha256 为 64，sha384/sha512 为 128）。

use super::digest::Engine;

/// 增量 HMAC 引擎：吸收消息后非破坏性求 MAC（可重复 digest）。
#[derive(Debug, Clone)]
pub(crate) struct HmacEngine {
    /// 底层摘要算法
    algo: super::digest::Algo,
    /// 内层摘要引擎（吸收消息中）
    inner: Engine,
    /// 外层常量：opad 异或后的密钥块
    okey_pad: Vec<u8>,
}

impl HmacEngine {
    /// 以原始密钥字节构造 HMAC 引擎（密钥长于分组时先摘要，对齐 RFC 2104 §2）。
    pub(crate) fn new(algo: super::digest::Algo, key: &[u8]) -> Self {
        let block = algo.block_len();
        let mut k0 = vec![0u8; block];
        if key.len() > block {
            let mut h = Engine::new(algo);
            h.update(key);
            k0[..algo.output_len()].copy_from_slice(&h.finalize());
        } else {
            k0[..key.len()].copy_from_slice(key);
        }
        let ipad = [0x36u8; 128];
        let mut opad = [0x5cu8; 128];
        let mut ikey_pad = vec![0u8; block];
        for i in 0..block {
            ikey_pad[i] = k0[i] ^ ipad[i];
            opad[i] ^= k0[i];
        }
        let mut inner = Engine::new(algo);
        inner.update(&ikey_pad);
        Self {
            algo,
            inner,
            okey_pad: opad[..block].to_vec(),
        }
    }

    /// 吸收消息片段。
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// 求 MAC（非破坏性：状态保持，可重复调用）。
    pub(crate) fn finalize(&self) -> Vec<u8> {
        let inner_sum = self.inner.finalize();
        let mut outer = Engine::new(self.algo);
        outer.update(&self.okey_pad);
        outer.update(&inner_sum);
        outer.finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::super::digest::Algo;
    use super::super::digest::to_hex;
    use super::HmacEngine;

    /// RFC 4231 HMAC-SHA-256 测试用例 1/2（与 RFC 2202 SHA-1/MD5 用例 1/2 同输入）。
    #[test]
    fn rfc4231_vectors() {
        // TC1：key = 0x0b × 20，data = "Hi There"
        let key1 = [0x0bu8; 20];
        // TC2：key = "Jefe"，data = "what do ya want for nothing?"
        let data2 = b"what do ya want for nothing?";
        let cases: &[(Algo, &[u8], &[u8], &str)] = &[
            (
                Algo::Sha256,
                &key1,
                b"Hi There",
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                Algo::Sha256,
                b"Jefe",
                data2,
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                Algo::Sha384,
                &key1,
                b"Hi There",
                "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6",
            ),
            (
                Algo::Sha384,
                b"Jefe",
                data2,
                "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
            ),
            (
                Algo::Sha512,
                &key1,
                b"Hi There",
                "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
            ),
            (
                Algo::Sha512,
                b"Jefe",
                data2,
                "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
            ),
            (
                Algo::Sha1,
                &key1,
                b"Hi There",
                "b617318655057264e28bc0b6fb378c8ef146be00",
            ),
            (
                Algo::Sha1,
                b"Jefe",
                data2,
                "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79",
            ),
        ];
        for (algo, key, data, want) in cases {
            let mut h = HmacEngine::new(*algo, key);
            h.update(data);
            assert_eq!(&to_hex(&h.finalize()), want, "{algo:?} TC 不匹配");
        }
    }

    /// RFC 2202 HMAC-MD5 测试用例 1/2。
    #[test]
    fn rfc2202_md5_vectors() {
        let cases: &[(Vec<u8>, &[u8], &str)] = &[
            (
                vec![0x0bu8; 16],
                b"Hi There",
                "9294727a3638bb1c13f48ef8158bfc9d",
            ),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?",
                "750c783e6ab0b503eaa86e310a5db738",
            ),
        ];
        for (key, data, want) in cases {
            let mut h = HmacEngine::new(Algo::Md5, key);
            h.update(data);
            assert_eq!(to_hex(&h.finalize()), *want);
        }
    }

    /// 长密钥（超过分组长）先摘要的分支：RFC 4231 TC6（key 长度 131 字节）。
    #[test]
    fn rfc4231_long_key() {
        let key: Vec<u8> = vec![0xaa; 131];
        let data = b"Test Using Larger Than Block-Size Key - Hash Key First";
        let cases: &[(Algo, &str)] = &[
            (
                Algo::Sha256,
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                Algo::Sha384,
                "4ece084485813e9088d2c63a041bc5b44f9ef1012a2b588f3cd11f05033ac4c60c2ef6ab4030fe8296248df163f44952",
            ),
            (
                Algo::Sha512,
                "80b24263c7c1a3ebb71493c1dd7be8b49b46d1f41b4aeec1121b013783f8f3526b56d037e05f2598bd0fd2215d6a1e5295e64f73f63f0aec8b915a985d786598",
            ),
        ];
        for (algo, want) in cases {
            let mut h = HmacEngine::new(*algo, &key);
            h.update(data);
            assert_eq!(to_hex(&h.finalize()), *want, "{algo:?} TC6 不匹配");
        }
    }

    /// finalize 幂等 + 增量喂入一致性。
    #[test]
    fn idempotent_and_incremental() {
        let mut h = HmacEngine::new(Algo::Sha256, b"secret");
        h.update(b"hello ");
        let first = h.finalize();
        assert_eq!(h.finalize(), first);
        h.update(b"world");
        let mut h2 = HmacEngine::new(Algo::Sha256, b"secret");
        h2.update(b"hello world");
        assert_eq!(h.finalize(), h2.finalize());
    }
}
