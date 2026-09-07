//! 编码辅助：标准 Base64（RFC 4648，含 `=` 填充）与 PEM 文本封装。

/// Base64 编码（标准字母表 + `=` 填充，对齐 Go `base64Encode` 输出）。
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(TABLE[(n >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() >= 2 {
            out.push(TABLE[(n >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() >= 3 {
            out.push(TABLE[n as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Base64 解码（忽略空白与 `=`，非法字符返回 `None`）。
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some(u32::from(b - b'A')),
            b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for b in clean {
        acc = (acc << 6) | val(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// PEM 编码：`-----BEGIN <label>-----` + 64 字符分行 Base64 + `-----END ...-----`。
pub(crate) fn pem_encode(label: &str, der: &[u8]) -> String {
    let b64 = base64_encode(der);
    let mut text = String::with_capacity(b64.len() + b64.len() / 64 + 64);
    text.push_str(&format!("-----BEGIN {label}-----\n"));
    let bytes = b64.as_bytes();
    for chunk in bytes.chunks(64) {
        text.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

/// PEM 解码：取首个 `-----BEGIN <label>-----` 段的 Base64 内容。
pub(crate) fn pem_decode(text: &str) -> Option<Vec<u8>> {
    let begin = text.find("-----BEGIN ")?;
    let label_start = begin + "-----BEGIN ".len();
    let label_end = text[label_start..].find("-----")? + label_start;
    let _label = &text[label_start..label_end];
    let body_start = label_end + 5;
    let end = text[body_start..].find("-----END")? + body_start;
    base64_decode(&text[body_start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10 官方 Base64 向量。
    #[test]
    fn rfc4648_base64_vectors() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, want) in cases {
            assert_eq!(base64_encode(input.as_bytes()), *want);
            assert_eq!(base64_decode(want).as_deref(), Some(input.as_bytes()));
        }
    }

    /// PEM 往返（含 64 字符换行）。
    #[test]
    fn pem_roundtrip() {
        let der: Vec<u8> = (0..100u8).collect();
        let text = pem_encode("CERTIFICATE", &der);
        assert!(text.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(text.ends_with("-----END CERTIFICATE-----\n"));
        assert_eq!(pem_decode(&text), Some(der));
    }
}
