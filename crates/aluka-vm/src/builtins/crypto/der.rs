//! 最小 ASN.1 DER 解码器 + X.509 证书 / RSA 私钥（PKCS#1/PKCS#8）解析。
//!
//! 仅覆盖 crypto 探针所需的证书字段（serialNumber、issuer/subject DN、
//! validity、SPKI、basicConstraints、SAN、extKeyUsage），不追求全量 X.509。

use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// DER 基础读取
// ---------------------------------------------------------------------------

/// DER TLV（tag + 内容切片）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tlv<'a> {
    /// ASN.1 tag 字节（不含长度）
    pub tag: u8,
    /// 内容字节
    pub content: &'a [u8],
}

impl<'a> Tlv<'a> {
    /// 以构造类型展开子 TLV 序列（生命周期跟随底层 DER 数据）。
    fn children(&self) -> Vec<Tlv<'a>> {
        let mut reader = DerReader::new(self.content);
        reader.read_all()
    }
}

/// DER 序列读取器。
pub(crate) struct DerReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    /// 以 DER 字节构造。
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 读取下一个 TLV；越界/长度非法返回 `None`。
    pub(crate) fn next(&mut self) -> Option<Tlv<'a>> {
        if self.pos + 2 > self.data.len() {
            return None;
        }
        let tag = self.data[self.pos];
        self.pos += 1;
        let first = self.data[self.pos];
        self.pos += 1;
        let len = if first < 0x80 {
            first as usize
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 || self.pos + n > self.data.len() {
                return None;
            }
            let mut l = 0usize;
            for b in &self.data[self.pos..self.pos + n] {
                l = (l << 8) | usize::from(*b);
            }
            self.pos += n;
            l
        };
        if self.pos + len > self.data.len() {
            return None;
        }
        let content = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Some(Tlv { tag, content })
    }

    /// 读取剩余全部 TLV。
    pub(crate) fn read_all(&mut self) -> Vec<Tlv<'a>> {
        let mut out = Vec::new();
        while let Some(tlv) = self.next() {
            out.push(tlv);
        }
        out
    }
}

/// 顶层解析：要求整段为一个 SEQUENCE 并返回其子 TLV。
fn top_sequence(data: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut r = DerReader::new(data);
    let seq = r.next()?;
    if seq.tag != 0x30 {
        return None;
    }
    Some(seq.children())
}

// ---------------------------------------------------------------------------
// OID / 字符串 / 时间
// ---------------------------------------------------------------------------

/// OID 内容字节 → 点分十进制串。
pub(crate) fn oid_to_string(content: &[u8]) -> String {
    if content.is_empty() {
        return String::new();
    }
    // 首字节 = arc1 * 40 + arc2（超界时尽力拆分，兼容高位 arc）
    let first = u32::from(content[0]);
    let (a, b) = if first < 80 {
        (first / 40, first % 40)
    } else {
        (2, first - 80)
    };
    let mut out = format!("{a}.{b}");
    let mut acc: u64 = 0;
    for &byte in &content[1..] {
        acc = (acc << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            let _ = write!(out, ".{acc}");
            acc = 0;
        }
    }
    out
}

/// DN 属性 OID → Node 短名（对齐 Go `x509OIDShortNames`）。
fn oid_short_name(oid: &str) -> &str {
    match oid {
        "2.5.4.3" => "CN",
        "2.5.4.4" => "SN",
        "2.5.4.5" => "serialNumber",
        "2.5.4.6" => "C",
        "2.5.4.7" => "L",
        "2.5.4.8" => "ST",
        "2.5.4.9" => "street",
        "2.5.4.10" => "O",
        "2.5.4.11" => "OU",
        "2.5.4.12" => "title",
        "2.5.4.15" => "businessCategory",
        "2.5.4.17" => "postalCode",
        "2.5.4.42" => "GN",
        "2.5.4.97" => "organizationIdentifier",
        "0.9.2342.19200300.100.1.1" => "UID",
        "0.9.2342.19200300.100.1.25" => "DC",
        "1.2.840.113549.1.9.1" => "emailAddress",
        _ => oid,
    }
}

/// DN 值字节 → 字符串（UTF8String/PrintableString/IA5String 均按字节直取）。
fn dn_value_string(tlv: &Tlv<'_>) -> String {
    String::from_utf8_lossy(tlv.content).into_owned()
}

/// Name（SEQUENCE OF SET OF SEQ{OID, value}）→ Node `TAG=value\n...` 格式。
pub(crate) fn name_to_string(name: &Tlv<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for set in name.children() {
        for atv in set.children() {
            let fields = atv.children();
            if fields.len() < 2 {
                continue;
            }
            let oid = oid_to_string(fields[0].content);
            parts.push(format!(
                "{}={}",
                oid_short_name(&oid),
                dn_value_string(&fields[1])
            ));
        }
    }
    parts.join("\n")
}

/// DN → `{TAG: value}` 对象字段序列（toLegacyObject 用，保持证书序）。
pub(crate) fn name_to_pairs(name: &Tlv<'_>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for set in name.children() {
        for atv in set.children() {
            let fields = atv.children();
            if fields.len() < 2 {
                continue;
            }
            let oid = oid_to_string(fields[0].content);
            pairs.push((oid_short_name(&oid).to_owned(), dn_value_string(&fields[1])));
        }
    }
    pairs
}

/// 月份名（Go `Jan _2` 布局）。
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// ASN.1 时间（UTCTime/GeneralizedTime）→ Node 风格
/// `"Sep  4 16:00:07 2026 GMT"`（日空格补齐两位）。
pub(crate) fn time_to_string(tlv: &Tlv<'_>) -> String {
    let text = String::from_utf8_lossy(tlv.content).into_owned();
    let b = text.as_bytes();
    let (year, rest): (i64, &[u8]) = match tlv.tag {
        // GeneralizedTime：YYYYMMDDHHMMSSZ
        0x18 => (parse_digits(&b[..4]).unwrap_or(0) as i64, &b[4..]),
        // UTCTime：YYMMDDHHMMSSZ；00-49 → 20xx，50-99 → 19xx
        _ => {
            let yy = parse_digits(&b[..2]).unwrap_or(0);
            let year = if yy >= 50 {
                1900 + yy as i64
            } else {
                2000 + yy as i64
            };
            (year, &b[2..])
        }
    };
    let month = parse_digits(&rest[..2]).unwrap_or(1).clamp(1, 12);
    let day = parse_digits(&rest[2..4]).unwrap_or(1);
    let hour = parse_digits(&rest[4..6]).unwrap_or(0);
    let minute = parse_digits(&rest[6..8]).unwrap_or(0);
    let second = parse_digits(&rest[8..10]).unwrap_or(0);
    format!(
        "{} {:2} {hour:02}:{minute:02}:{second:02} {year} GMT",
        MONTHS[(month - 1) as usize],
        day
    )
}

/// 十进制 ASCII 数字串 → u64。
fn parse_digits(b: &[u8]) -> Option<u64> {
    b.iter().try_fold(0u64, |acc, c| {
        c.is_ascii_digit().then(|| acc * 10 + u64::from(c - b'0'))
    })
}

/// DER INTEGER 内容 → 去符号位大端字节（对齐 `big.Int.Bytes()`）。
fn integer_magnitude(content: &[u8]) -> Vec<u8> {
    let mut out = content.to_vec();
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    if out.len() == 1 && out[0] == 0 {
        return Vec::new();
    }
    out
}

// ---------------------------------------------------------------------------
// X.509 证书解析
// ---------------------------------------------------------------------------

/// 解析后的证书字段（对齐 Go `x509CertToValue` 的输出面）。
#[derive(Debug, Clone)]
pub(crate) struct ParsedCert {
    /// 原始 DER
    pub der: Vec<u8>,
    /// 序列号（去符号位大端）
    pub serial: Vec<u8>,
    /// 颁发者 DN 串
    pub issuer: String,
    /// 主体 DN 串
    pub subject: String,
    /// subject DN 字段对（toLegacyObject）
    pub subject_pairs: Vec<(String, String)>,
    /// issuer DN 字段对
    pub issuer_pairs: Vec<(String, String)>,
    /// 起始时间（Node 风格 GMT 串）
    pub valid_from: String,
    /// 截止时间
    pub valid_to: String,
    /// 是否 CA（basicConstraints）
    pub is_ca: bool,
    /// SAN 串（`DNS:x, DNS:*.y, IP:z`；无 SAN 为空）
    pub san: String,
    /// SAN DNS 名单（checkHost 用）
    pub dns_names: Vec<String>,
    /// SAN IP 名单（点分/压缩串）
    pub ip_addresses: Vec<String>,
    /// 扩展密钥用途 OID 列表
    pub ext_key_usage: Vec<String>,
    /// SPKI DER（publicKey.raw）
    pub spki_der: Vec<u8>,
    /// RSA 模数（去符号位大端；非 RSA 证书为空）
    pub modulus: Vec<u8>,
    /// RSA 公钥指数
    pub exponent: u64,
}

/// 解析 X.509 证书 DER。
pub(crate) fn parse_certificate(der: &[u8]) -> Option<ParsedCert> {
    let top = top_sequence(der)?;
    if top.len() < 3 {
        return None;
    }
    let tbs = top[0];
    if tbs.tag != 0x30 {
        return None;
    }
    let fields = tbs.children();
    let mut idx = 0usize;
    // [0] EXPLICIT version（可选）
    if let Some(first) = fields.first() {
        if first.tag == 0xa0 {
            idx = 1;
        }
    }
    let serial_tlv = fields.get(idx)?;
    if serial_tlv.tag != 0x02 {
        return None;
    }
    let serial = integer_magnitude(serial_tlv.content);
    // sigAlg（跳过）→ issuer → validity → subject → spki
    let issuer = fields.get(idx + 2)?;
    let validity = fields.get(idx + 3)?;
    let subject = fields.get(idx + 4)?;
    let spki = fields.get(idx + 5)?;
    if issuer.tag != 0x30 || validity.tag != 0x30 || subject.tag != 0x30 || spki.tag != 0x30 {
        return None;
    }
    let times = validity.children();
    if times.len() < 2 {
        return None;
    }
    let valid_from = time_to_string(&times[0]);
    let valid_to = time_to_string(&times[1]);

    // SPKI：SEQUENCE { AlgorithmIdentifier, BIT STRING }
    let spki_der = tbs_slice(spki);
    let mut modulus: Vec<u8> = Vec::new();
    let mut exponent: u64 = 0;
    let spki_parts = spki.children();
    if spki_parts.len() >= 2 && spki_parts[1].tag == 0x03 && !spki_parts[1].content.is_empty() {
        // BIT STRING 首字节为未用位数（RSA 恒为 0）
        let key_der = &spki_parts[1].content[1..];
        if let Some(rsa_parts) = top_sequence(key_der) {
            if rsa_parts.len() >= 2 && rsa_parts[0].tag == 0x02 && rsa_parts[1].tag == 0x02 {
                modulus = integer_magnitude(rsa_parts[0].content);
                exponent = u64::from_le_bytes({
                    let mag = integer_magnitude(rsa_parts[1].content);
                    let mut b = [0u8; 8];
                    for (i, x) in mag.iter().take(8).enumerate() {
                        b[i] = *x;
                    }
                    b
                });
            }
        }
    }

    // 扩展 [3] EXPLICIT：SEQUENCE OF Extension
    let mut is_ca = false;
    let mut san = String::new();
    let mut dns_names: Vec<String> = Vec::new();
    let mut ip_addresses: Vec<String> = Vec::new();
    let mut ext_key_usage: Vec<String> = Vec::new();
    for field in fields.iter().skip(idx + 6) {
        if field.tag != 0xa3 {
            continue;
        }
        for ext_seq in field.children() {
            if ext_seq.tag != 0x30 {
                continue;
            }
            for ext in ext_seq.children() {
                let parts = ext.children();
                if parts.len() < 2 || parts[0].tag != 0x06 {
                    continue;
                }
                let oid = oid_to_string(parts[0].content);
                // 扩展值为 OCTET STRING（内嵌一段 DER）
                let value = parts.iter().rev().find(|p| p.tag == 0x04);
                let Some(value) = value else { continue };
                match oid.as_str() {
                    "2.5.29.19" => {
                        // basicConstraints：SEQUENCE { ca BOOLEAN DEFAULT FALSE, ... }
                        if let Some(bc) = top_sequence(value.content) {
                            if let Some(flag) = bc.first() {
                                if flag.tag == 0x01 && !flag.content.is_empty() {
                                    is_ca = flag.content[0] != 0;
                                }
                            }
                        }
                    }
                    "2.5.29.17" => {
                        let (text, dns, ips) = parse_san(value.content);
                        san = text;
                        dns_names = dns;
                        ip_addresses = ips;
                    }
                    "2.5.29.37" => {
                        if let Some(eku) = top_sequence(value.content) {
                            for oid_tlv in eku {
                                if oid_tlv.tag == 0x06 {
                                    ext_key_usage.push(oid_to_string(oid_tlv.content));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Some(ParsedCert {
        der: der.to_vec(),
        serial,
        issuer: name_to_string(issuer),
        subject: name_to_string(subject),
        subject_pairs: name_to_pairs(subject),
        issuer_pairs: name_to_pairs(issuer),
        valid_from,
        valid_to,
        is_ca,
        san,
        dns_names,
        ip_addresses,
        ext_key_usage,
        spki_der,
        modulus,
        exponent,
    })
}

/// SPKI 的原始 DER 切片还原（TLV 头 + 内容）。
fn tbs_slice(tlv: &Tlv<'_>) -> Vec<u8> {
    let len = tlv.content.len();
    let mut out = vec![tlv.tag];
    let l = len;
    if l < 0x80 {
        out.push(l as u8);
    } else if l <= 0xff {
        out.push(0x81);
        out.push(l as u8);
    } else {
        out.push(0x82);
        out.push((l >> 8) as u8);
        out.push((l & 0xff) as u8);
    }
    out.extend_from_slice(tlv.content);
    out
}

/// 解析 SAN 扩展值 → `("DNS:x, DNS:*.y, IP:z", dns 名单, ip 名单)`。
fn parse_san(content: &[u8]) -> (String, Vec<String>, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut dns_names: Vec<String> = Vec::new();
    let mut ip_addresses: Vec<String> = Vec::new();
    if let Some(seq) = top_sequence(content) {
        for name in seq {
            match name.tag {
                // [2] dNSName（IA5String）
                0x82 => {
                    let dns = String::from_utf8_lossy(name.content).into_owned();
                    parts.push(format!("DNS:{dns}"));
                    dns_names.push(dns);
                }
                // [7] iPAddress（OCTET STRING）
                0x87 => match name.content.len() {
                    4 => {
                        let text = format!(
                            "{}.{}.{}.{}",
                            name.content[0], name.content[1], name.content[2], name.content[3]
                        );
                        parts.push(format!("IP:{text}"));
                        ip_addresses.push(text);
                    }
                    16 => {
                        let text = ipv6_string(name.content);
                        parts.push(format!("IP:{text}"));
                        ip_addresses.push(text);
                    }
                    _ => {}
                },
                // [1] rfc822Name
                0x81 => {
                    parts.push(format!("email:{}", String::from_utf8_lossy(name.content)));
                }
                // [6] uniformResourceIdentifier
                0x86 => {
                    parts.push(format!("URI:{}", String::from_utf8_lossy(name.content)));
                }
                _ => {}
            }
        }
    }
    (parts.join(", "), dns_names, ip_addresses)
}

/// 16 字节 IPv6 → RFC 5952 压缩小写串（零段折叠）。
fn ipv6_string(b: &[u8]) -> String {
    let groups: Vec<u16> = (0..8)
        .map(|i| u16::from_be_bytes([b[i * 2], b[i * 2 + 1]]))
        .collect();
    // 找最长全零段
    let (mut best_start, mut best_len) = (usize::MAX, 0usize);
    let (mut cur_start, mut cur_len) = (0usize, 0usize);
    for (i, g) in groups.iter().enumerate() {
        if *g == 0 {
            if cur_len == 0 {
                cur_start = i;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_start = cur_start;
                best_len = cur_len;
            }
        } else {
            cur_len = 0;
        }
    }
    if best_len < 2 {
        best_start = usize::MAX;
    }
    let mut out = String::new();
    let mut i = 0usize;
    while i < groups.len() {
        if i == best_start {
            out.push_str("::");
            i += best_len;
        } else {
            if !out.is_empty() && !out.ends_with(':') {
                out.push(':');
            }
            let _ = write!(out, "{:x}", groups[i]);
            i += 1;
        }
    }
    if out.is_empty() {
        out.push_str("::");
    }
    out
}

// ---------------------------------------------------------------------------
// RSA 私钥（PKCS#1 / PKCS#8）解析
// ---------------------------------------------------------------------------

/// 解析出的 RSA 私钥公开面（模数 + 指数，供 verify/checkPrivateKey 比对）。
#[derive(Debug, Clone)]
pub(crate) struct RsaPrivateKeyInfo {
    /// 模数（去符号位大端）
    pub modulus: Vec<u8>,
    /// 公钥指数
    pub exponent: u64,
}

/// 从 PEM 文本解析 RSA 私钥（PKCS#1 `RSA PRIVATE KEY` / PKCS#8 `PRIVATE KEY`）。
pub(crate) fn parse_private_key_pem(text: &str) -> Option<RsaPrivateKeyInfo> {
    let der = super::enc::pem_decode(text)?;
    parse_private_key_der(&der)
}

/// 从 DER 解析 RSA 私钥（先试 PKCS#1，再试 PKCS#8 包裹）。
pub(crate) fn parse_private_key_der(der: &[u8]) -> Option<RsaPrivateKeyInfo> {
    if let Some(info) = parse_pkcs1(der) {
        return Some(info);
    }
    // PKCS#8：SEQUENCE { version INTEGER, AlgorithmIdentifier SEQ, privateKey OCTET STRING }
    let top = top_sequence(der)?;
    if top.len() >= 3 && top[0].tag == 0x02 && top[2].tag == 0x04 {
        return parse_pkcs1(top[2].content);
    }
    None
}

/// 解析 PKCS#1 RSAPrivateKey：SEQUENCE { version, n, e, d, ... }。
fn parse_pkcs1(der: &[u8]) -> Option<RsaPrivateKeyInfo> {
    let top = top_sequence(der)?;
    if top.len() < 3 || top[0].tag != 0x02 {
        return None;
    }
    let n = integer_magnitude(top[1].content);
    let e_bytes = integer_magnitude(top[2].content);
    if n.is_empty() || e_bytes.is_empty() || e_bytes.len() > 8 {
        return None;
    }
    let mut exponent = 0u64;
    for b in &e_bytes {
        exponent = (exponent << 8) | u64::from(*b);
    }
    Some(RsaPrivateKeyInfo {
        modulus: n,
        exponent,
    })
}

/// 从 PEM 文本解析 SubjectPublicKeyInfo（RSA 公钥）。
pub(crate) fn parse_public_key_pem(text: &str) -> Option<RsaPrivateKeyInfo> {
    let der = super::enc::pem_decode(text)?;
    parse_spki(&der)
}

/// 从 DER 解析 SPKI 公钥。
pub(crate) fn parse_spki(der: &[u8]) -> Option<RsaPrivateKeyInfo> {
    let top = top_sequence(der)?;
    let bit_string = top.get(1)?;
    if bit_string.tag != 0x03 || bit_string.content.is_empty() {
        return None;
    }
    let key_der = &bit_string.content[1..];
    let rsa = top_sequence(key_der)?;
    if rsa.len() < 2 || rsa[0].tag != 0x02 || rsa[1].tag != 0x02 {
        return None;
    }
    let modulus = integer_magnitude(rsa[0].content);
    let e_bytes = integer_magnitude(rsa[1].content);
    let mut exponent = 0u64;
    for b in &e_bytes {
        exponent = (exponent << 8) | u64::from(*b);
    }
    Some(RsaPrivateKeyInfo { modulus, exponent })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::crypto::digest::{Algo, Engine, to_hex};

    /// 手工构造的最小 DER：SEQUENCE { INTEGER(0x014a), BOOLEAN(true) }。
    #[test]
    fn der_reader_basics() {
        let der = [0x30, 0x07, 0x02, 0x02, 0x01, 0x4a, 0x01, 0x01, 0xff];
        let mut r = DerReader::new(&der);
        let seq = r.next().unwrap();
        assert_eq!(seq.tag, 0x30);
        let children = seq.children();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].content, &[0x01, 0x4a]);
        assert_eq!(integer_magnitude(children[0].content), vec![0x01, 0x4a]);
        assert_eq!(children[1].content, &[0xff]);
    }

    /// DER INTEGER 去符号位：前导 0x00 必须剥除（对齐 big.Int.Bytes）。
    #[test]
    fn integer_sign_byte() {
        assert_eq!(integer_magnitude(&[0x00, 0x8f, 0xff]), vec![0x8f, 0xff]);
        assert_eq!(integer_magnitude(&[0x7f]), vec![0x7f]);
        assert!(integer_magnitude(&[0x00]).is_empty());
    }

    /// OID 解码：RSA 公钥算法 OID 与典型 SAN OID。
    #[test]
    fn oid_decoding() {
        // 1.2.840.113549.1.1.1（rsaEncryption）
        let oid = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
        assert_eq!(oid_to_string(&oid), "1.2.840.113549.1.1.1");
        // 2.5.29.17（subjectAltName）
        assert_eq!(oid_to_string(&[0x55, 0x1d, 0x11]), "2.5.29.17");
    }

    /// 时间格式化：UTCTime / GeneralizedTime → Node 风格 GMT 串。
    #[test]
    fn time_formatting() {
        let utc = Tlv {
            tag: 0x17,
            content: b"260904160007Z",
        };
        assert_eq!(time_to_string(&utc), "Sep  4 16:00:07 2026 GMT");
        let generalized = Tlv {
            tag: 0x18,
            content: b"20360901160007Z",
        };
        assert_eq!(time_to_string(&generalized), "Sep  1 16:00:07 2036 GMT");
        // 单数日补空格对齐（Go `Jan _2` 布局）
        let utc2 = Tlv {
            tag: 0x17,
            content: b"260901000001Z",
        };
        assert_eq!(time_to_string(&utc2), "Sep  1 00:00:01 2026 GMT");
    }

    /// 指纹（DER → 冒号分隔大写 hex）对齐已知 sha1 摘要。
    #[test]
    fn fingerprint_format() {
        let der = b"hello world";
        let mut h = Engine::new(Algo::Sha1);
        h.update(der);
        let sum = h.finalize();
        let fp = sum
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        assert!(fp.contains(':'));
        assert_eq!(fp.len(), 20 * 3 - 1);
        assert_eq!(to_hex(&sum), "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
    }

    /// IPv6 压缩串（零段折叠）。
    #[test]
    fn ipv6_formatting() {
        let mut b = [0u8; 16];
        b[15] = 1;
        assert_eq!(ipv6_string(&b), "::1");
        let mut b2 = [0u8; 16];
        b2[0] = 0x20;
        b2[1] = 0x01;
        b2[2] = 0x0d;
        b2[3] = 0xb8;
        b2[15] = 0x0a;
        assert_eq!(ipv6_string(&b2), "2001:db8::a");
    }
}
