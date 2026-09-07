//! AES 分组密码（FIPS 197）纯 Rust 实现：128/192/256 位密钥的加解密块函数。
//!
//! 状态按 FIPS 列主序存储（`state[r + 4c]`），密钥扩展按标准 Key Expansion。

/// AES S 盒。
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// AES 逆 S 盒。
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

/// 轮常量（Rcon 首字节；Key Expansion 至多使用 10 个）。
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

/// GF(2^8) 乘法（模 x^8 + x^4 + x^3 + x + 1）。
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

/// AES 密钥调度后的块密码：预扩展轮密钥，随后逐块加解密。
#[derive(Debug, Clone)]
pub(crate) struct AesBlock {
    /// 轮数（10/12/14）
    rounds: usize,
    /// 扩展轮密钥（4 * (rounds + 1) 个 32 位字）
    round_keys: Vec<u32>,
}

impl AesBlock {
    /// 以 16/24/32 字节密钥构造（其余长度返回 `None`）。
    pub(crate) fn new(key: &[u8]) -> Option<Self> {
        let nk = match key.len() {
            16 => 4usize,
            24 => 6,
            32 => 8,
            _ => return None,
        };
        let rounds = nk + 6;
        let total_words = 4 * (rounds + 1);
        let mut w = vec![0u32; total_words];
        for (i, word) in w.iter_mut().take(nk).enumerate() {
            let b = &key[i * 4..i * 4 + 4];
            *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in nk..total_words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                temp = temp.rotate_left(8);
                temp = sub_word(temp);
                temp ^= (RCON[i / nk - 1] as u32) << 24;
            } else if nk > 6 && i % nk == 4 {
                temp = sub_word(temp);
            }
            w[i] = w[i - nk] ^ temp;
        }
        Some(Self {
            rounds,
            round_keys: w,
        })
    }

    /// 加密单个 16 字节块（就地覆盖）。
    pub(crate) fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[..4]);
        for round in 1..self.rounds {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round * 4..round * 4 + 4]);
        }
        sub_bytes(block);
        shift_rows(block);
        add_round_key(
            block,
            &self.round_keys[self.rounds * 4..self.rounds * 4 + 4],
        );
    }

    /// 解密单个 16 字节块（就地覆盖，FIPS 197 §5.3 InvCipher）。
    pub(crate) fn decrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(
            block,
            &self.round_keys[self.rounds * 4..self.rounds * 4 + 4],
        );
        for round in (1..self.rounds).rev() {
            inv_shift_rows(block);
            inv_sub_bytes(block);
            add_round_key(block, &self.round_keys[round * 4..round * 4 + 4]);
            inv_mix_columns(block);
        }
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(block, &self.round_keys[..4]);
    }
}

/// 字替换（SubWord）。
fn sub_word(w: u32) -> u32 {
    let b = w.to_be_bytes();
    u32::from_be_bytes([
        SBOX[b[0] as usize],
        SBOX[b[1] as usize],
        SBOX[b[2] as usize],
        SBOX[b[3] as usize],
    ])
}

/// AddRoundKey：第 c 列异或轮密钥字 `keys[c]`（FIPS 布局 `state[r + 4c]`）。
fn add_round_key(block: &mut [u8; 16], keys: &[u32]) {
    for (c, word) in keys.iter().enumerate() {
        let k = word.to_be_bytes();
        for (r, byte) in k.iter().enumerate() {
            block[c * 4 + r] ^= byte;
        }
    }
}

/// SubBytes。
fn sub_bytes(block: &mut [u8; 16]) {
    for b in block.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

/// InvSubBytes。
fn inv_sub_bytes(block: &mut [u8; 16]) {
    for b in block.iter_mut() {
        *b = INV_SBOX[*b as usize];
    }
}

/// ShiftRows（状态行左环移；存储布局 `state[r + 4c]`）。
fn shift_rows(block: &mut [u8; 16]) {
    for r in 1..4 {
        for _ in 0..r {
            block.rotate_left_within_row(r);
        }
    }
}

/// InvShiftRows（状态行右环移）。
fn inv_shift_rows(block: &mut [u8; 16]) {
    for r in 1..4 {
        for _ in 0..r {
            block.rotate_right_within_row(r);
        }
    }
}

/// 行内左旋一格的辅助（trait 方法见下方实现）。
trait RowRotate {
    fn rotate_left_within_row(&mut self, row: usize);
    fn rotate_right_within_row(&mut self, row: usize);
}

impl RowRotate for [u8; 16] {
    fn rotate_left_within_row(&mut self, row: usize) {
        let first = self[row];
        for c in 0..3 {
            self[row + 4 * c] = self[row + 4 * (c + 1)];
        }
        self[row + 12] = first;
    }

    fn rotate_right_within_row(&mut self, row: usize) {
        let last = self[row + 12];
        for c in (1..4).rev() {
            self[row + 4 * c] = self[row + 4 * (c - 1)];
        }
        self[row] = last;
    }
}

/// MixColumns：每列乘 {02,03,01,01} 循环矩阵（列 c = `block[4c..4c+4]`）。
fn mix_columns(block: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [
            block[4 * c],
            block[4 * c + 1],
            block[4 * c + 2],
            block[4 * c + 3],
        ];
        block[4 * c] = gf_mul(col[0], 2) ^ gf_mul(col[1], 3) ^ col[2] ^ col[3];
        block[4 * c + 1] = col[0] ^ gf_mul(col[1], 2) ^ gf_mul(col[2], 3) ^ col[3];
        block[4 * c + 2] = col[0] ^ col[1] ^ gf_mul(col[2], 2) ^ gf_mul(col[3], 3);
        block[4 * c + 3] = gf_mul(col[0], 3) ^ col[1] ^ col[2] ^ gf_mul(col[3], 2);
    }
}

/// InvMixColumns：每列乘 {0e,0b,0d,09} 循环矩阵。
fn inv_mix_columns(block: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [
            block[4 * c],
            block[4 * c + 1],
            block[4 * c + 2],
            block[4 * c + 3],
        ];
        block[4 * c] =
            gf_mul(col[0], 14) ^ gf_mul(col[1], 11) ^ gf_mul(col[2], 13) ^ gf_mul(col[3], 9);
        block[4 * c + 1] =
            gf_mul(col[0], 9) ^ gf_mul(col[1], 14) ^ gf_mul(col[2], 11) ^ gf_mul(col[3], 13);
        block[4 * c + 2] =
            gf_mul(col[0], 13) ^ gf_mul(col[1], 9) ^ gf_mul(col[2], 14) ^ gf_mul(col[3], 11);
        block[4 * c + 3] =
            gf_mul(col[0], 11) ^ gf_mul(col[1], 13) ^ gf_mul(col[2], 9) ^ gf_mul(col[3], 14);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 十六进制串 → 16 字节块。
    fn hex(s: &str) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("合法 hex");
        }
        out
    }

    /// 16 字节块 → 十六进制串。
    fn unhex(block: &[u8; 16]) -> String {
        let mut s = String::new();
        for b in block {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// 任意长度十六进制串 → 字节向量（密钥构造用）。
    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("合法 hex"))
            .collect()
    }

    /// FIPS 197 附录 C 官方向量（128/192/256），加解密往返。
    #[test]
    fn fips197_appendix_c() {
        let pt = hex("00112233445566778899aabbccddeeff");
        let cases: &[(&str, &str)] = &[
            (
                "000102030405060708090a0b0c0d0e0f",
                "69c4e0d86a7b0430d8cdb78070b4c55a",
            ),
            (
                "000102030405060708090a0b0c0d0e0f1011121314151617",
                "dda97ca4864cdfe06eaf70a0ec0d7191",
            ),
            (
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "8ea2b7ca516745bfeafc49904b496089",
            ),
        ];
        for (key_hex, ct_hex) in cases {
            let aes = AesBlock::new(&hex_bytes(key_hex)).expect("合法密钥长度");
            let mut block = pt;
            aes.encrypt_block(&mut block);
            assert_eq!(unhex(&block), *ct_hex, "密钥 {key_hex} 加密不符");
            aes.decrypt_block(&mut block);
            assert_eq!(block, pt, "解密必须还原明文");
        }
    }
}
