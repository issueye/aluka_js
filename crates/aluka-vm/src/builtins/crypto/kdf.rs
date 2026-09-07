//! 密钥派生函数（KDF）纯 Rust 实现：PBKDF2（RFC 2898）、scrypt（RFC 7914）、
//! HKDF（RFC 5869）。
//!
//! 对齐 Node.js 22 LTS 标准（`nodecrypto/crypto_kdf.go`）：
//! - `pbkdf2_key`：digest 仅支持 sha1/sha256/sha512（与 Go 一致，不含 md5/sha384）；
//! - `scrypt`：默认参数 N=16384, r=8, p=1，N 必须为 >1 的 2 的幂；
//! - `hkdf_key`：digest 走统一摘要引擎（md5/sha1/sha256/sha384/sha512）。

use super::digest::Algo;
use super::hmac::HmacEngine;

/// PBKDF2（RFC 2898 §5.2）：`dk = F(P, S, c, 1) || F(P, S, c, 2) ...` 截取 `dk_len`。
pub(crate) fn pbkdf2_key(
    algo: Algo,
    password: &[u8],
    salt: &[u8],
    iterations: i64,
    dk_len: i64,
) -> Result<Vec<u8>, String> {
    if iterations <= 0 {
        return Err("pbkdf2: iterations must be positive".to_owned());
    }
    if dk_len <= 0 {
        return Err("pbkdf2: keylen must be positive".to_owned());
    }
    let iterations = iterations as u32;
    let dk_len = dk_len as usize;
    let mut dk = Vec::with_capacity(dk_len);
    let mut block_index: u32 = 1;
    while dk.len() < dk_len {
        // U1 = PRF(P, S || INT_32_BE(i))
        let mut prf = HmacEngine::new(algo, password);
        prf.update(salt);
        prf.update(&block_index.to_be_bytes());
        let mut u = prf.finalize();
        let mut t = u.clone();
        // U2..Uc 逐轮异或
        for _ in 1..iterations {
            let mut prf = HmacEngine::new(algo, password);
            prf.update(&u);
            u = prf.finalize();
            for (tb, ub) in t.iter_mut().zip(&u) {
                *tb ^= ub;
            }
        }
        dk.extend_from_slice(&t);
        block_index = block_index.wrapping_add(1);
    }
    dk.truncate(dk_len);
    Ok(dk)
}

/// scrypt（RFC 7914）：`SMix` 基于 Salsa20/8 核心的 ROMix。
/// 参数校验对齐 golang.org/x/crypto/scrypt（N 为 >1 的 2 的幂，r/p 为正）。
pub(crate) fn scrypt_key(
    password: &[u8],
    salt: &[u8],
    n: i64,
    r: i64,
    p: i64,
    dk_len: usize,
) -> Result<Vec<u8>, String> {
    if n <= 1 || (n & (n - 1)) != 0 {
        return Err("scrypt: N must be > 1 and a power of 2".to_owned());
    }
    if r <= 0 {
        return Err("scrypt: r must be > 0".to_owned());
    }
    if p <= 0 {
        return Err("scrypt: p must be > 0".to_owned());
    }
    let (n, r, p) = (n as usize, r as usize, p as usize);
    let block_bytes = 128usize
        .checked_mul(r)
        .ok_or_else(|| "scrypt: parameters too large".to_owned())?;
    let total = block_bytes
        .checked_mul(p)
        .ok_or_else(|| "scrypt: parameters too large".to_owned())?;
    let mut b = pbkdf2_key(Algo::Sha256, password, salt, 1, total as i64)?;
    for chunk in b.chunks_mut(block_bytes) {
        smix(chunk, r, n);
    }
    let out = pbkdf2_key(Algo::Sha256, password, &b, 1, dk_len as i64)?;
    Ok(out)
}

/// SMix（RFC 7914 §5）：顺序内存硬 ROMix。
fn smix(b: &mut [u8], r: usize, n: usize) {
    let len128r = 128 * r;
    let mut v = vec![0u8; len128r * n];
    let mut xy = vec![0u8; 256 * r];
    xy[..len128r].copy_from_slice(b);
    // V[0..n) 顺序记录
    for chunk in v.chunks_mut(len128r) {
        chunk.copy_from_slice(&xy[..len128r]);
        block_mix(&mut xy, r);
    }
    // 按 Integerify(X) mod N 回访 V 并二次混合
    let j_base = (2 * r - 1) * 64;
    for _ in 0..n {
        let j = u32::from_le_bytes([xy[j_base], xy[j_base + 1], xy[j_base + 2], xy[j_base + 3]])
            as usize
            % n;
        let target = v[j * len128r..(j + 1) * len128r].to_vec();
        for (x, t) in xy[..len128r].iter_mut().zip(target) {
            *x ^= t;
        }
        block_mix(&mut xy, r);
    }
    b.copy_from_slice(&xy[..len128r]);
}

/// scryptBlockMix（RFC 7914 §4）：Salsa20/8 交替混合 + 奇偶交错重排。
fn block_mix(b: &mut [u8], r: usize) {
    let words = 32 * r; // 2r 个 Salsa 块，每块 16 字
    let mut x: Vec<u32> = Vec::with_capacity(words);
    for chunk in b[..256 * r].chunks_exact(4) {
        x.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    let mut y = vec![0u32; words];
    // X = B[2r-1]，Y[i] = Salsa8(X ^ B[i])
    let mut prev: [u32; 16] = x[(2 * r - 1) * 16..(2 * r) * 16].try_into().expect("16 字");
    for i in 0..2 * r {
        let mut t: [u32; 16] = x[i * 16..(i + 1) * 16].try_into().expect("16 字");
        for (tb, pb) in t.iter_mut().zip(prev) {
            *tb ^= pb;
        }
        prev = salsa20_8(&t);
        y[i * 16..(i + 1) * 16].copy_from_slice(&prev);
    }
    // B' = (Y[0], Y[2], ..., Y[2r-2], Y[1], Y[3], ..., Y[2r-1])
    let mut byte_idx = 0usize;
    for i in (0..2 * r).step_by(2) {
        let seg = le_bytes(&y[i * 16..(i + 1) * 16]);
        b[byte_idx..byte_idx + 64].copy_from_slice(&seg);
        byte_idx += 64;
    }
    for i in (1..2 * r).step_by(2) {
        let seg = le_bytes(&y[i * 16..(i + 1) * 16]);
        b[byte_idx..byte_idx + 64].copy_from_slice(&seg);
        byte_idx += 64;
    }
}

/// Salsa20/8 核心：8 轮（4 列轮 + 4 行轮交替）+ 原输入加和。
fn salsa20_8(input: &[u32; 16]) -> [u32; 16] {
    // Salsa20 双轮的列轮 / 行轮索引序列：(a, b, c, d)
    //   x[b] ^= (x[a] + x[d]) <<< 7
    //   x[c] ^= (x[b] + x[a]) <<< 9
    //   x[d] ^= (x[c] + x[b]) <<< 13
    //   x[a] ^= (x[d] + x[c]) <<< 18
    const COLUMNS: [(usize, usize, usize, usize); 4] =
        [(0, 4, 8, 12), (5, 9, 13, 1), (10, 14, 2, 6), (15, 3, 7, 11)];
    const ROWS: [(usize, usize, usize, usize); 4] =
        [(0, 1, 2, 3), (5, 6, 7, 4), (10, 11, 8, 9), (15, 12, 13, 14)];
    let mut x = *input;
    for round in 0..8 {
        let table = if round % 2 == 0 { &COLUMNS } else { &ROWS };
        for &(a, b, c, d) in table {
            quarter(&mut x, a, b, c, d);
        }
    }
    let mut out = [0u32; 16];
    for (o, (xi, ii)) in out.iter_mut().zip(x.iter().zip(input)) {
        *o = xi.wrapping_add(*ii);
    }
    out
}

/// Salsa20 quarterround 四步旋转加。
fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[b] ^= x[a].wrapping_add(x[d]).rotate_left(7);
    x[c] ^= x[b].wrapping_add(x[a]).rotate_left(9);
    x[d] ^= x[c].wrapping_add(x[b]).rotate_left(13);
    x[a] ^= x[d].wrapping_add(x[c]).rotate_left(18);
}

/// u32 切片 → 小端字节序列。
fn le_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// HKDF（RFC 5869）：extract + expand，`info` 可空。
pub(crate) fn hkdf_key(
    algo: Algo,
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    dk_len: usize,
) -> Result<Vec<u8>, String> {
    if dk_len == 0 {
        return Err("hkdf: keylen must be positive".to_owned());
    }
    let hash_len = algo.output_len();
    if dk_len > 255 * hash_len {
        return Err("hkdf: keylen too large".to_owned());
    }
    // PRK = HMAC-Hash(salt, IKM)（RFC 允许空 salt 以零填充到 hash_len）
    let mut prf = HmacEngine::new(algo, salt);
    prf.update(ikm);
    let prk = prf.finalize();
    // OKM = T(1) || T(2) ... 截取 dk_len
    let mut okm = Vec::with_capacity(dk_len);
    let mut t: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while okm.len() < dk_len {
        let mut mac = HmacEngine::new(algo, &prk);
        mac.update(&t);
        mac.update(info);
        mac.update(&[counter]);
        t = mac.finalize();
        okm.extend_from_slice(&t);
        counter = counter.wrapping_add(1);
    }
    okm.truncate(dk_len);
    Ok(okm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6070 PBKDF2-HMAC-SHA1 官方向量。
    #[test]
    fn rfc6070_pbkdf2_sha1() {
        let cases: &[(&str, &str, i64, i64, &str)] = &[
            (
                "password",
                "salt",
                1,
                20,
                "0c60c80f961f0e71f3a9b524af6012062fe037a6",
            ),
            (
                "password",
                "salt",
                2,
                20,
                "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
            ),
            (
                "password",
                "salt",
                4096,
                20,
                "4b007901b765489abead49d926f721d065a429c1",
            ),
            (
                "passwordPASSWORDpassword",
                "saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                25,
                "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038",
            ),
        ];
        for (pw, salt, iter, len, want) in cases {
            let dk = pbkdf2_key(Algo::Sha1, pw.as_bytes(), salt.as_bytes(), *iter, *len).unwrap();
            assert_eq!(super::super::digest::to_hex(&dk), *want);
        }
    }

    /// RFC 7914 §11 PBKDF2-HMAC-SHA-256 官方向量。
    #[test]
    fn rfc7914_pbkdf2_sha256() {
        let cases: &[(&str, &str, i64, i64, &str)] = &[
            (
                "passwd",
                "salt",
                1,
                64,
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
            ),
            (
                "Password",
                "NaCl",
                80000,
                64,
                "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d",
            ),
        ];
        for (pw, salt, iter, len, want) in cases {
            let dk = pbkdf2_key(Algo::Sha256, pw.as_bytes(), salt.as_bytes(), *iter, *len).unwrap();
            assert_eq!(super::super::digest::to_hex(&dk), *want);
        }
    }

    /// RFC 7914 §12 scrypt 官方向量（第二向量 N=16384，验证内存硬路径）。
    #[test]
    fn rfc7914_scrypt_vectors() {
        let dk1 = scrypt_key(b"password", b"NaCl", 1024, 8, 16, 64).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&dk1),
            "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640"
        );
        let dk2 = scrypt_key(b"pleaseletmein", b"SodiumChloride", 16384, 8, 1, 64).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&dk2),
            "7023bdcb3afd7348461c06cd81fd38ebfda8fbba904f8e3ea9b543f6545da1f2d5432955613f0fcf62d49705242a9af9e61e85dc0d651e40dfcf017b45575887"
        );
    }

    /// RFC 5869 HKDF 官方向量（SHA-256 用例 1/2/3 与 SHA-1 用例 7）。
    #[test]
    fn rfc5869_hkdf_vectors() {
        // 用例 1
        let ikm = vec![0x0bu8; 22];
        let salt: Vec<u8> = (0x00..=0x0c).collect();
        let info: Vec<u8> = (0xf0..=0xf9).collect();
        let okm = hkdf_key(Algo::Sha256, &ikm, &salt, &info, 42).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        // 用例 3：salt 与 info 均空
        let okm = hkdf_key(Algo::Sha256, &ikm, &[], &[], 42).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
        // 用例 2：长输入
        let ikm2: Vec<u8> = (0x00..=0x4f).collect();
        let salt2: Vec<u8> = (0x60..=0xaf).collect();
        let info2: Vec<u8> = (0xb0..=0xff).collect();
        let okm = hkdf_key(Algo::Sha256, &ikm2, &salt2, &info2, 82).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&okm),
            "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87"
        );
        // 用例 7：SHA-1（与用例 1 相同 salt/info）
        let okm = hkdf_key(Algo::Sha1, &ikm, &salt, &info, 42).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&okm),
            "d6000ffb5b50bd3970b260017798fb9c8df9ce2e2c16b6cd709cca07dc3cf9cf26d6c6d750d0aaf5ac94"
        );
        // 用例 6：SHA-1，salt 与 info 均空
        let okm6 = hkdf_key(Algo::Sha1, &ikm, &[], &[], 42).unwrap();
        assert_eq!(
            super::super::digest::to_hex(&okm6),
            "0ac1af7002b3d761d1e55298da9d0506b9ae52057220a306e07b6b87e8df21d0ea00033de03984d34918"
        );
    }

    /// KDF 参数错误路径。
    #[test]
    fn kdf_error_paths() {
        assert_eq!(
            pbkdf2_key(Algo::Sha1, b"p", b"s", 0, 8).unwrap_err(),
            "pbkdf2: iterations must be positive"
        );
        assert_eq!(
            pbkdf2_key(Algo::Sha1, b"p", b"s", 1, 0).unwrap_err(),
            "pbkdf2: keylen must be positive"
        );
        assert_eq!(
            scrypt_key(b"p", b"s", 3, 1, 1, 8).unwrap_err(),
            "scrypt: N must be > 1 and a power of 2"
        );
    }
}
