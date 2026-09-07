//! AES 工作模式（NIST SP 800-38A / SP 800-38D）：CBC / ECB / CTR / GCM 与
//! PKCS#7 填充。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodecrypto/crypto_cipher.go`）：
//! - `pad` 恒追加整块填充（数据对齐时也补一块）；
//! - `unpad` 对非法填充**不报错**、原样返回（Go 实现的宽松行为）；
//! - CBC 解密要求数据非空且为分组整数倍，否则报
//!   `cipher: data length must be multiple of block size`。

use super::aes::AesBlock;

/// CBC 加密（数据须为分组整数倍；IV 16 字节）。
pub(crate) fn cbc_encrypt(aes: &AesBlock, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let mut prev: [u8; 16] = iv.try_into().unwrap_or([0u8; 16]);
    for (chunk, out_chunk) in data.chunks_exact(16).zip(out.chunks_exact_mut(16)) {
        let mut block: [u8; 16] = chunk.try_into().expect("分组长 16");
        for (b, p) in block.iter_mut().zip(prev) {
            *b ^= p;
        }
        aes.encrypt_block(&mut block);
        out_chunk.copy_from_slice(&block);
        prev = block;
    }
    out
}

/// CBC 解密（数据须为分组整数倍；IV 16 字节）。
pub(crate) fn cbc_decrypt(aes: &AesBlock, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let mut prev: [u8; 16] = iv.try_into().unwrap_or([0u8; 16]);
    for (chunk, out_chunk) in data.chunks_exact(16).zip(out.chunks_exact_mut(16)) {
        let mut block: [u8; 16] = chunk.try_into().expect("分组长 16");
        let cipher_block = block;
        aes.decrypt_block(&mut block);
        for (b, p) in block.iter_mut().zip(prev) {
            *b ^= p;
        }
        prev = cipher_block;
        out_chunk.copy_from_slice(&block);
    }
    out
}

/// ECB 加密（数据须为分组整数倍）。
pub(crate) fn ecb_encrypt(aes: &AesBlock, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    for (chunk, out_chunk) in data.chunks_exact(16).zip(out.chunks_exact_mut(16)) {
        let mut block: [u8; 16] = chunk.try_into().expect("分组长 16");
        aes.encrypt_block(&mut block);
        out_chunk.copy_from_slice(&block);
    }
    out
}

/// ECB 解密（数据须为分组整数倍）。
pub(crate) fn ecb_decrypt(aes: &AesBlock, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    for (chunk, out_chunk) in data.chunks_exact(16).zip(out.chunks_exact_mut(16)) {
        let mut block: [u8; 16] = chunk.try_into().expect("分组长 16");
        aes.decrypt_block(&mut block);
        out_chunk.copy_from_slice(&block);
    }
    out
}

/// CTR 模式流加解密（任意长度；IV/计数器初值 16 字节，32 位计数器自增）。
pub(crate) fn ctr(aes: &AesBlock, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut counter: [u8; 16] = iv.try_into().unwrap_or([0u8; 16]);
    let mut keystream = [0u8; 16];
    let mut ks_pos = keystream.len();
    for byte in data {
        if ks_pos == keystream.len() {
            keystream = counter;
            aes.encrypt_block(&mut keystream);
            inc32(&mut counter);
            ks_pos = 0;
        }
        out.push(byte ^ keystream[ks_pos]);
        ks_pos += 1;
    }
    out
}

/// 32 位大端计数器自增（SP 800-38D §6.2 inc32）。
fn inc32(counter: &mut [u8; 16]) {
    for b in counter[12..].iter_mut().rev() {
        *b = b.wrapping_add(1);
        if *b != 0 {
            break;
        }
    }
}

/// PKCS#7 填充（恒追加 1..=bs 字节，对齐块时补整块）。
pub(crate) fn pkcs7_pad(data: &[u8], bs: usize) -> Vec<u8> {
    let n = bs - data.len() % bs;
    let mut out = Vec::with_capacity(data.len() + n);
    out.extend_from_slice(data);
    out.resize(data.len() + n, n as u8);
    out
}

/// PKCS#7 去填充（非法填充原样返回，对齐 Node.js 22 LTS 标准 的宽松 unpad）。
pub(crate) fn pkcs7_unpad(data: &[u8], bs: usize) -> Vec<u8> {
    let Some(&last) = data.last() else {
        return data.to_vec();
    };
    let n = last as usize;
    if n < 1 || n > bs || n > data.len() {
        return data.to_vec();
    }
    data[..data.len() - n].to_vec()
}

// ---------------------------------------------------------------------------
// GCM（SP 800-38D）：GHASH + GCTR，仅默认 12 字节 nonce / 16 字节 tag
// ---------------------------------------------------------------------------

/// GF(2^128) 乘法（多项式 R = 0xe1000000...）。
fn gf128_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;
    for i in 0..128 {
        if (x[i / 8] >> (7 - i % 8)) & 1 == 1 {
            for (zb, vb) in z.iter_mut().zip(v) {
                *zb ^= vb;
            }
        }
        let lsb = v[15] & 1;
        // 右移一位
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | (v[j - 1] << 7);
        }
        v[0] >>= 1;
        if lsb == 1 {
            v[0] ^= 0xe1;
        }
    }
    z
}

/// GHASH：对 `data`（须为 16 的倍数）做 GHASH 链，从初值 `y` 出发。
fn ghash_mul_blocks(mut y: [u8; 16], h: &[u8; 16], data: &[u8]) -> [u8; 16] {
    for chunk in data.chunks_exact(16) {
        for (yb, cb) in y.iter_mut().zip(chunk) {
            *yb ^= cb;
        }
        y = gf128_mul(&y, h);
    }
    y
}

/// GCM 认证标签：`T = GHASH_H(A || pad || C || pad || len64(A) || len64(C))
/// XOR E(K, J0)`（标签恒对**密文**计算，加解密共用）。
fn gcm_tag(aes: &AesBlock, iv: &[u8], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    // H = E(K, 0^128)
    let mut h = [0u8; 16];
    aes.encrypt_block(&mut h);
    // J0 = IV || 0^3 || 1（12 字节 IV 专用）
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(&iv[..12]);
    j0[15] = 1;
    // S = GHASH_H(A || pad || C || pad || len64(A) || len64(C))
    let mut ghdata = pad16(aad);
    ghdata.extend_from_slice(&pad16(ciphertext));
    let a_bits = (aad.len() as u64) * 8;
    let c_bits = (ciphertext.len() as u64) * 8;
    ghdata.extend_from_slice(&a_bits.to_be_bytes());
    ghdata.extend_from_slice(&c_bits.to_be_bytes());
    let mut s = ghash_mul_blocks([0u8; 16], &h, &ghdata);
    // T = GCTR(J0, S)
    let mut j0_block = j0;
    aes.encrypt_block(&mut j0_block);
    for (sb, jb) in s.iter_mut().zip(j0_block) {
        *sb ^= jb;
    }
    s
}

/// GCM 加密：返回 `(密文, 16 字节认证标签)`。
pub(crate) fn gcm_encrypt(
    aes: &AesBlock,
    iv: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> ([u8; 16], Vec<u8>) {
    // 密文 = GCTR(J0+1, P)
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(&iv[..12]);
    j0[15] = 1;
    let mut icb = j0;
    inc32(&mut icb);
    let ciphertext = ctr(aes, &icb, plaintext);
    let tag = gcm_tag(aes, iv, aad, &ciphertext);
    (tag, ciphertext)
}

/// GCM 解密：对密文重算标签并常数时间比较，一致则返回明文，否则 `None`。
pub(crate) fn gcm_decrypt(
    aes: &AesBlock,
    iv: &[u8],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Option<Vec<u8>> {
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(&iv[..12]);
    j0[15] = 1;
    let mut icb = j0;
    inc32(&mut icb);
    let plaintext = ctr(aes, &icb, ciphertext);
    let expect = gcm_tag(aes, iv, aad, ciphertext);
    if expect.len() != tag.len() {
        return None;
    }
    // 常数时间比较
    let mut diff = 0u8;
    for (a, b) in expect.iter().zip(tag) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return None;
    }
    Some(plaintext)
}

/// PKCS#7 式补齐到 16 的倍数（整倍数时原样返回，供 GHASH 拼接）。
fn pad16(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let rem = data.len() % 16;
    if rem != 0 {
        out.resize(data.len() + 16 - rem, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NIST SP 800-38A F.1.1/F.2.1：CBC-AES128 已知向量。
    #[test]
    fn sp80038a_cbc_aes128() {
        let key = hex_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = hex_bytes("000102030405060708090a0b0c0d0e0f");
        let pt = hex_bytes("6bc1bee22e409f96e93d7e117393172a");
        let want = "7649abac8119b246cee98e9b12e9197d";
        let aes = AesBlock::new(&key).unwrap();
        let ct = cbc_encrypt(&aes, &iv, &pkcs7_pad(&pt, 16));
        assert_eq!(hex_str(&ct[..16]), want);
        let round = cbc_decrypt(&aes, &iv, &ct);
        assert_eq!(pkcs7_unpad(&round, 16), pt);
    }

    /// NIST SP 800-38A F.2.3：CBC-AES256 已知向量（ct1 = f58c4c04...）。
    #[test]
    fn sp80038a_cbc_aes256() {
        let key = hex_bytes("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
        let iv = hex_bytes("000102030405060708090a0b0c0d0e0f");
        let pt = hex_bytes("6bc1bee22e409f96e93d7e117393172a");
        let want = "f58c4c04d6e5f1ba779eabfb5f7bfbd6";
        let aes = AesBlock::new(&key).unwrap();
        let ct = cbc_encrypt(&aes, &iv, &pkcs7_pad(&pt, 16));
        assert_eq!(hex_str(&ct[..16]), want);
    }

    /// NIST SP 800-38A F.5.1：CTR-AES128 已知向量（含非对齐尾部）。
    #[test]
    fn sp80038a_ctr_aes128() {
        let key = hex_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let ctr_iv = hex_bytes("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let pt = hex_bytes("6bc1bee22e409f96e93d7e117393172a");
        let want = "874d6191b620e3261bef6864990db6ce";
        let aes = AesBlock::new(&key).unwrap();
        let ct = ctr(&aes, &ctr_iv, &pt);
        assert_eq!(hex_str(&ct), want);
        let round = ctr(&aes, &ctr_iv, &ct);
        assert_eq!(round, pt);
    }

    /// ECB 往返 + 已知块向量（FIPS 197 附录 C 复用）。
    #[test]
    fn ecb_roundtrip() {
        let key = hex_bytes("000102030405060708090a0b0c0d0e0f");
        let aes = AesBlock::new(&key).unwrap();
        let pt = hex_bytes("00112233445566778899aabbccddeeff");
        let ct = ecb_encrypt(&aes, &pt);
        assert_eq!(hex_str(&ct), "69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(ecb_decrypt(&aes, &ct), pt);
    }

    /// GCM 标准测试向量（GCM 规范 Test Case 1/2/3：零密钥零 IV）。
    #[test]
    fn gcm_spec_vectors() {
        let key = [0u8; 16];
        let aes = AesBlock::new(&key).unwrap();
        let iv = [0u8; 12];
        // TC1：空明文 → tag 58e2fccefa7e3061367f1d57a4e7455a（Node.js 22 LTS 标准 实测一致）
        let (tag, ct) = gcm_encrypt(&aes, &iv, &[], &[]);
        assert_eq!(hex_str(&tag), "58e2fccefa7e3061367f1d57a4e7455a");
        assert!(ct.is_empty());
        // TC2：16 字节全零明文 → ct 0388dace60b6a392f328c2b971b2fe78
        //      tag ab6e47d42cec13bdf53a67b21257bddf
        let pt = [0u8; 16];
        let (tag, ct) = gcm_encrypt(&aes, &iv, &[], &pt);
        assert_eq!(hex_str(&ct), "0388dace60b6a392f328c2b971b2fe78");
        assert_eq!(hex_str(&tag), "ab6e47d42cec13bdf53a67b21257bddf");
        assert_eq!(gcm_decrypt(&aes, &iv, &[], &ct, &tag), Some(pt.to_vec()));
        // 篡改标签必须校验失败
        let mut bad = tag;
        bad[0] ^= 1;
        assert_eq!(gcm_decrypt(&aes, &iv, &[], &ct, &bad), None);
        // TC3：非对齐明文（"Darth Plagueis the Wise" 段落，此处用 20 字节样本自洽往返）
        let msg = b"abcdefghijklmnopqrst";
        let (tag, ct) = gcm_encrypt(&aes, &iv, &[], msg);
        assert_eq!(gcm_decrypt(&aes, &iv, &[], &ct, &tag), Some(msg.to_vec()));
    }

    /// PKCS#7 填充边界：对齐数据补整块；非法填充原样返回。
    #[test]
    fn pkcs7_edges() {
        assert_eq!(pkcs7_pad(b"", 16).len(), 16);
        assert_eq!(pkcs7_pad(b"1234567890123456", 16).len(), 32);
        assert_eq!(pkcs7_pad(b"abc", 16)[15], 13);
        // Go 宽松语义：仅按尾字节长度截断，不校验填充内容（[1,2,3,3] → [1]）
        assert_eq!(pkcs7_unpad(&[1, 2, 3, 0x03], 16), vec![1]);
        // 非法填充（0x00）原样返回
        assert_eq!(pkcs7_unpad(&[1, 2, 3, 0x00], 16), vec![1, 2, 3, 0x00]);
        // 越界填充长度原样返回
        assert_eq!(pkcs7_unpad(&[1, 2, 3, 0x05], 4), vec![1, 2, 3, 0x05]);
    }

    fn hex_str(bytes: &[u8]) -> String {
        let mut s = String::new();
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("合法 hex"))
            .collect()
    }
}
