//! HTTP/1.1 报文的纯 std 手写解析与生成。
//!
//! 与 Node.js 22 LTS 标准（`nodehttp`，底层为 Go net/http）的线上行为对齐：
//! - 请求/响应均支持 `Content-Length` 与 `Transfer-Encoding: chunked` 两种定界；
//! - 客户端请求带未知长度 body 时用 chunked（Go `io.NopCloser` 语义）；
//! - 服务端小响应自动补 `Content-Length` 与嗅探的 `Content-Type`（Go 缓冲
//!   writer 行为），bodyless 状态码（1xx/204/304）不写二者。

use std::time::{SystemTime, UNIX_EPOCH};

/// 请求头集合（解析结果）：小写名 → 值列表（保持出现顺序）。
pub(crate) type HeaderList = Vec<(String, Vec<String>)>;

/// 解析出的 HTTP 请求头。
pub(crate) struct RequestHead {
    /// 请求方法（如 `GET`）
    pub method: String,
    /// 请求目标（RequestURI，如 `/abc`）
    pub target: String,
    /// 请求行声明的 HTTP 版本（`(major, minor)`）。服务端据此判定默认
    /// keep-alive（HTTP/1.1 默认保持、HTTP/1.0 默认关闭）与响应体定界
    /// （HTTP/1.0 客户端不写 `Content-Length`/chunked，改以关连接定界）。
    pub version: (u8, u8),
    /// 全部头（小写名，保序）
    pub headers: HeaderList,
    /// Content-Length（无则 None）
    pub content_length: Option<u64>,
    /// 是否 chunked 编码
    pub chunked: bool,
}

/// 解析出的 HTTP 响应头。
pub(crate) struct ResponseHead {
    /// 状态码
    pub status: u16,
    /// 状态短语整体（Go `resp.Status`，如 `"200 OK"`）
    pub status_message: String,
    /// 全部头（小写名，保序）
    pub headers: HeaderList,
    /// Content-Length（无则 None）
    pub content_length: Option<u64>,
    /// 是否 chunked 编码
    pub chunked: bool,
}

/// 在缓冲中定位 `\r\n\r\n`（头区结束），返回其起始下标。
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 解析头区块（不含结尾空行）。解析失败返回 None。
fn parse_head_block(
    head: &[u8],
    is_request: bool,
) -> Option<(Vec<String>, HeaderList, Option<u64>, bool)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let start_line = lines.next()?.to_string();
    let mut headers: HeaderList = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let lname = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if let Some(entry) = headers.iter_mut().find(|(n, _)| *n == lname) {
            entry.1.push(value);
        } else {
            headers.push((lname, vec![value]));
        }
    }
    let get = |k: &str| -> Option<String> {
        headers
            .iter()
            .find(|(n, _)| n == k)
            .and_then(|(_, v)| v.first().cloned())
    };
    let content_length = get("content-length").and_then(|v| v.trim().parse::<u64>().ok());
    let chunked = get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let _ = is_request;
    Some((vec![start_line], headers, content_length, chunked))
}

/// 从请求头行解析 `RequestHead`。
fn request_head(
    start_line: &str,
    headers: HeaderList,
    content_length: Option<u64>,
    chunked: bool,
) -> Option<RequestHead> {
    let mut parts = start_line.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    Some(RequestHead {
        method,
        target,
        version: parse_http_version(start_line),
        headers,
        content_length,
        chunked,
    })
}

/// 解析起始行声明的 HTTP 版本（`"HTTP/1.0"` → `(1, 0)`）。
/// 请求缺乏／畸形版本声明时按 HTTP/1.1 兜底（llhttp 对请求行的容错口径）。
pub(crate) fn parse_http_version(start_line: &str) -> (u8, u8) {
    let Some(token) = start_line.split(' ').next_back() else {
        return (1, 1);
    };
    let Some(rest) = token.strip_prefix("HTTP/") else {
        return (1, 1);
    };
    let mut parts = rest.split('.');
    let major = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(1);
    let minor = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(1);
    (major, minor)
}

/// 头值是否含指定 token（按非 token 字符切分后整词忽略大小写比较）。
///
/// 等价于 Node 的 `RE_CONN_CLOSE = /(?:^|\W)close(?:$|\W)/i`
/// （`_http_outgoing.js:93`）以及 llhttp 对 `Connection` 值的匹配口径：
/// `"keep-alive, close"` 同时含两个 token，`"close"` 只含 `close`。
pub(crate) fn conn_token(value: &str, token: &str) -> bool {
    value
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .any(|t| t.eq_ignore_ascii_case(token))
}

/// 取某头的首值（小写名；缺失返回 None）。
pub(crate) fn header_value(headers: &HeaderList, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| v.first().cloned())
}

/// 尝试从缓冲取走一个完整请求（头 + 体），成功时消费已用字节。
/// chunked 体按分块解析到终止块；不完整则返回 None（等下一轮数据）。
pub(crate) fn try_take_request(buf: &mut Vec<u8>) -> Option<(RequestHead, Vec<u8>)> {
    let head_end = find_header_end(buf)?;
    let (lines, headers, content_length, chunked) = parse_head_block(&buf[..head_end], true)?;
    let head = request_head(&lines[0], headers, content_length, chunked)?;
    let mut pos = head_end + 4;
    let body: Vec<u8>;
    if head.chunked {
        let (consumed, chunk_body) = take_chunked_with_body(buf, pos)?;
        pos = consumed;
        body = chunk_body;
    } else if let Some(cl) = head.content_length {
        if buf.len() < pos + cl as usize {
            return None;
        }
        body = buf[pos..pos + cl as usize].to_vec();
        pos += cl as usize;
    } else {
        body = Vec::new();
    }
    buf.drain(..pos);
    Some((head, body))
}

/// 解析 chunked 体：返回（消费结束位置，拼接后的体字节）。未完整返回 None。
/// 帧游走同时被 `builtins::global::fetch` 的响应定界判定复用（见 `fetch.rs`）。
pub(crate) fn take_chunked_with_body(buf: &[u8], mut pos: usize) -> Option<(usize, Vec<u8>)> {
    let mut body = Vec::new();
    loop {
        let line_end = find_crlf(buf, pos)?;
        let size_str = std::str::from_utf8(&buf[pos..line_end]).ok()?;
        let size = usize::from_str_radix(size_str.trim().split(';').next()?.trim(), 16).ok()?;
        pos = line_end + 2;
        if size == 0 {
            // 终止块之后可能有 trailer 头，找到空行收尾（允许缺失）
            if let Some(te) = find_header_end(&buf[pos..]) {
                pos += te + 4;
            } else if buf.len() >= pos + 2 && &buf[pos..pos + 2] == b"\r\n" {
                pos += 2;
            }
            return Some((pos, body));
        }
        // saturating_add：对端可能声明超大 chunk-size，避免无符号溢出
        if pos.saturating_add(size).saturating_add(2) > buf.len() {
            return None;
        }
        body.extend_from_slice(&buf[pos..pos + size]);
        pos += size + 2; // 跳过块尾 CRLF
    }
}

/// 在 `buf[pos..]` 中找 `\r\n`，返回其起始下标（绝对）。
fn find_crlf(buf: &[u8], pos: usize) -> Option<usize> {
    if pos > buf.len() {
        return None;
    }
    buf[pos..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|i| i + pos)
}

/// 从缓冲解析响应。`eof` 表示对端已关闭（用于无 CL/TE 响应的到 EOF 定界）。
/// 返回（头, 体, 是否以 EOF 定界）。不完整返回 None。
pub(crate) fn try_take_response(
    buf: &mut Vec<u8>,
    eof: bool,
) -> Option<(ResponseHead, Vec<u8>, bool)> {
    let head_end = find_header_end(buf)?;
    let (lines, headers, content_length, chunked) = parse_head_block(&buf[..head_end], false)?;
    let status_line = &lines[0];
    // "HTTP/1.1 200 OK" → 状态码与短语（Go `resp.Status` 为含码整体 "200 OK"）
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next()?;
    let status: u16 = parts.next()?.parse().ok()?;
    let reason = parts.next().unwrap_or("");
    let status_message = format!("{status} {reason}");
    let head = ResponseHead {
        status,
        status_message,
        headers,
        content_length,
        chunked,
    };
    let mut pos = head_end + 4;
    let body: Vec<u8>;
    let mut eof_delimited = false;
    if head.chunked {
        let (consumed, chunk_body) = take_chunked_with_body(buf, pos)?;
        pos = consumed;
        body = chunk_body;
    } else if let Some(cl) = head.content_length {
        if buf.len() < pos + cl as usize {
            return None;
        }
        body = buf[pos..pos + cl as usize].to_vec();
        pos += cl as usize;
    } else if status_is_bodyless(head.status) {
        body = Vec::new();
    } else {
        // 无长度声明：等待连接关闭定界
        if !eof {
            return None;
        }
        body = buf[pos..].to_vec();
        pos = buf.len();
        eof_delimited = true;
    }
    buf.drain(..pos);
    Some((head, body, eof_delimited))
}

/// 状态码是否不允许 body（1xx / 204 / 304，Go `bodyAllowedForStatus` 反集）。
pub(crate) fn status_is_bodyless(status: u16) -> bool {
    (100..=199).contains(&status) || status == 204 || status == 304
}

/// 生成响应字节：状态行 + 头（按给定顺序）+ 空行 + 体。
pub(crate) fn serialize_response(
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + body.len());
    out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, status_reason(status)).as_bytes());
    for (name, value) in headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// 生成客户端请求字节：请求行 + 固定头 + 用户头 + 体定界。
/// 有 body 时用 chunked（Go 未知长度 body 语义），无 body 不写 CL/TE。
pub(crate) fn serialize_request(
    method: &str,
    target: &str,
    host_header: &str,
    user_headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + body.len());
    out.extend_from_slice(format!("{method} {target} HTTP/1.1\r\n").as_bytes());
    out.extend_from_slice(format!("Host: {host_header}\r\n").as_bytes());
    out.extend_from_slice(b"User-Agent: Go-http-client/1.1\r\n");
    out.extend_from_slice(b"Accept-Encoding: gzip\r\n");
    for (name, value) in user_headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    if !body.is_empty() {
        out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    }
    out.extend_from_slice(b"\r\n");
    if !body.is_empty() {
        out.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\r\n0\r\n\r\n");
    }
    out
}

/// 当前 UTC 时间的 HTTP 日期头（`Mon, 02 Jan 2006 15:04:05 GMT`）。
pub(crate) fn http_date_now() -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    let weekday = (days + 4).rem_euclid(7) as usize; // 1970-01-01 是周四
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday],
        day,
        MONTHS[(month - 1) as usize],
        year,
        hour,
        min,
        sec
    )
}

/// 天数 → (年, 月, 日)（Howard Hinnant `civil_from_days` 算法）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Go `sniff.sign` 的文本/二进制判定子集：非文本字节 → octet-stream，
/// 否则 text/plain（Go 对未匹配签名的纯文本 body 的默认嗅探结果）。
pub(crate) fn sniff_content_type(body: &[u8]) -> &'static str {
    let probe = &body[..body.len().min(512)];
    for &b in probe {
        if b <= 0x08 || b == 0x0B || (0x0E..=0x1A).contains(&b) || (0x1C..=0x1F).contains(&b) {
            return "application/octet-stream";
        }
    }
    "text/plain; charset=utf-8"
}

/// 状态码 → 标准短语（Go `http.StatusText`；未知名返回空串）。
pub(crate) fn status_reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}
