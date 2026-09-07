//! 统一摘要引擎：`md5` / `sha1` / `sha256` / `sha384` / `sha512` 增量计算门面。
//!
//! 对齐 Node.js 22 LTS 标准（`nodecrypto/crypto_hash.go` 的 `newDigest`）：算法名**精确
//! 匹配**（大小写敏感），未支持算法返回 `createHash: unsupported algorithm` 错误。
//! 引擎支持跨 `digest` 的非破坏性求值（`hash.Hash.Sum(nil)` 语义）。

use super::md5;
use super::sha1;
use super::sha2;

/// 摘要算法标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Algo {
    /// MD5（RFC 1321）
    Md5,
    /// SHA-1（FIPS 180-4）
    Sha1,
    /// SHA-256（FIPS 180-4）
    Sha256,
    /// SHA-384（FIPS 180-4）
    Sha384,
    /// SHA-512（FIPS 180-4）
    Sha512,
}

impl Algo {
    /// 按算法名精确解析（对齐 Go `newDigest` 的 switch：大小写敏感）。
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "md5" => Some(Self::Md5),
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            "sha384" => Some(Self::Sha384),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }

    /// 摘要输出长度（字节）。
    pub(crate) fn output_len(self) -> usize {
        match self {
            Self::Md5 => md5::OUTPUT_LEN,
            Self::Sha1 => sha1::OUTPUT_LEN,
            Self::Sha256 => sha2::SHA256_OUT,
            Self::Sha384 => sha2::SHA384_OUT,
            Self::Sha512 => sha2::SHA512_OUT,
        }
    }

    /// 压缩分组长度（字节）：md5/sha1/sha256 为 64，sha384/sha512 为 128。
    pub(crate) fn block_len(self) -> usize {
        match self {
            Self::Md5 | Self::Sha1 | Self::Sha256 => 64,
            Self::Sha384 | Self::Sha512 => sha2::SHA512_BLOCK,
        }
    }
}

/// 增量摘要引擎：缓冲不足一分组的尾部，`finalize` 非破坏性。
#[derive(Debug, Clone)]
pub(crate) struct Engine {
    /// 算法标识
    algo: Algo,
    /// 已吸收的总字节数
    total: u128,
    /// 32 位压缩状态（md5/sha1/sha256 用）
    words32: [u32; 8],
    /// 64 位压缩状态（sha384/sha512 用）
    words64: [u64; 8],
    /// 不足一分组的输入尾部缓冲
    tail: Vec<u8>,
}

impl Engine {
    /// 按算法创建引擎并写入初始状态。
    pub(crate) fn new(algo: Algo) -> Self {
        let mut engine = Self {
            algo,
            total: 0,
            words32: [0; 8],
            words64: [0; 8],
            tail: Vec::new(),
        };
        engine.reset_state();
        engine
    }

    /// 写入算法初始向量。
    fn reset_state(&mut self) {
        match self.algo {
            Algo::Md5 => self.words32[..4].copy_from_slice(&md5::initial_state()),
            Algo::Sha1 => self.words32[..5].copy_from_slice(&sha1::initial_state()),
            Algo::Sha256 => self.words32.copy_from_slice(&sha2::initial_state256()),
            Algo::Sha384 => self.words64.copy_from_slice(&sha2::initial_state384()),
            Algo::Sha512 => self.words64.copy_from_slice(&sha2::initial_state512()),
        }
    }

    /// 吸收任意长度输入（可分多次调用，语义与一次性计算完全一致）。
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u128);
        let block = self.algo.block_len();
        let mut data = data;
        // 先把旧尾部凑满一个分组
        if !self.tail.is_empty() {
            let need = block - self.tail.len();
            let take = need.min(data.len());
            self.tail.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.tail.len() == block {
                let full = std::mem::take(&mut self.tail);
                self.compress_block(&full);
            }
        }
        let mut chunks = data.chunks_exact(block);
        for chunk in chunks.by_ref() {
            self.compress_block(chunk);
        }
        self.tail.extend_from_slice(chunks.remainder());
    }

    /// 求摘要（非破坏性：引擎状态保持不变，可重复调用）。
    pub(crate) fn finalize(&self) -> Vec<u8> {
        let block = self.algo.block_len();
        // Merkle–Damgård 填充：0x80 + 零 + 长度字段，总长凑成分组整数倍
        let len_field: usize =
            if self.algo == Algo::Md5 || self.algo == Algo::Sha1 || self.algo == Algo::Sha256 {
                8
            } else {
                16
            };
        let bit_len = self.total.wrapping_mul(8);
        let pad_len = (block - (self.tail.len() + 1 + len_field) % block) % block;
        let mut padded = self.tail.clone();
        padded.push(0x80);
        padded.resize(padded.len() + pad_len, 0);
        if self.algo == Algo::Md5 {
            padded.extend_from_slice(&(bit_len as u64).to_le_bytes());
        } else {
            padded.extend_from_slice(&bit_len.to_be_bytes()[16 - len_field..]);
        }
        // 对克隆体压缩填充块（不污染引擎真实状态 → Sum(nil) 幂等语义）
        let mut clone = self.clone();
        for chunk in padded.chunks_exact(block) {
            clone.compress_block(chunk);
        }
        let mut out = Vec::with_capacity(self.algo.output_len());
        match self.algo {
            Algo::Md5 => {
                for w in &clone.words32[..4] {
                    out.extend_from_slice(&w.to_le_bytes());
                }
            }
            Algo::Sha1 => {
                for w in &clone.words32[..5] {
                    out.extend_from_slice(&w.to_be_bytes());
                }
            }
            Algo::Sha256 => {
                for w in &clone.words32[..8] {
                    out.extend_from_slice(&w.to_be_bytes());
                }
            }
            Algo::Sha384 | Algo::Sha512 => {
                let n = if self.algo == Algo::Sha384 { 6 } else { 8 };
                for w in &clone.words64[..n] {
                    out.extend_from_slice(&w.to_be_bytes());
                }
            }
        }
        out
    }

    /// 压缩一个完整分组（真实状态更新）。
    fn compress_block(&mut self, block: &[u8]) {
        match self.algo {
            Algo::Md5 => {
                // 显式借用再压缩：保证写入穿透到引擎状态（避免临时值歧义）
                let state: &mut [u32; 4] = (&mut self.words32[..4])
                    .try_into()
                    .expect("md5 状态为 4 字");
                md5::compress(state, block);
            }
            Algo::Sha1 => {
                let state: &mut [u32; 5] = (&mut self.words32[..5])
                    .try_into()
                    .expect("sha1 状态为 5 字");
                sha1::compress(state, block);
            }
            Algo::Sha256 => sha2::compress256(&mut self.words32, block),
            Algo::Sha384 | Algo::Sha512 => sha2::compress512(&mut self.words64, block),
        }
    }
}

/// 字节序列 → 小写十六进制串。
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 算法名必须精确匹配（大小写敏感，对齐 Node.js 22 LTS 标准）。
    #[test]
    fn algo_name_resolution() {
        for name in ["md5", "sha1", "sha256", "sha384", "sha512"] {
            assert!(Algo::from_name(name).is_some());
        }
        for name in ["SHA256", "sha", "", "sha512x"] {
            assert!(
                Algo::from_name(name).is_none(),
                "算法名必须精确匹配：{name}"
            );
        }
    }

    /// hex 编码辅助。
    #[test]
    fn hex_encoding() {
        assert_eq!(to_hex(&[0x00, 0xff, 0x10]), "00ff10");
    }

    /// digest 非破坏性：重复 finalize 结果一致，且后续 update 继续累积。
    #[test]
    fn finalize_is_idempotent() {
        let mut e = Engine::new(Algo::Sha256);
        e.update(b"ab");
        let first = e.finalize();
        assert_eq!(e.finalize(), first);
        e.update(b"cd");
        let mut expect = Engine::new(Algo::Sha256);
        expect.update(b"abcd");
        assert_eq!(e.finalize(), expect.finalize());
    }
}
