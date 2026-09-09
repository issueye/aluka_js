//! registry HTTP(S) 客户端（ureq + rustls，纯 Rust TLS）。
//!
//! npm 客户端只出站访问 registry：GET（packument / tarball），无认证面
//! （公共 registry 口径；私有源经 `--registry` 指定）。

use std::io::Read;
use std::time::Duration;

/// HTTP 客户端错误。
#[derive(Debug)]
pub enum HttpError {
    /// 网络 / TLS / 协议层失败
    Transport(String),
    /// registry 返回非成功状态码
    Status(u16, String),
    /// 响应体读取出错
    Body(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Transport(m) => write!(f, "网络错误: {m}"),
            HttpError::Status(code, url) => write!(f, "HTTP {code}: {url}"),
            HttpError::Body(m) => write!(f, "响应体错误: {m}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// registry 客户端（连接复用由 ureq Agent 内部管理）。
pub struct HttpClient {
    agent: ureq::Agent,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// 构建客户端（10s 连接超时；UA 标识 aluka-npm）。
    pub fn new() -> HttpClient {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .user_agent("aluka-npm/0.1 (+https://github.com/issueye/aluka_lang)")
            .build();
        HttpClient { agent }
    }

    /// GET 并读取全量响应体为字节（tarball / packument 通用）。
    ///
    /// gzip 响应由 ureq `gzip` 特性透明解压；跳转 ≤5 跳。
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let resp = self.agent.get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => HttpError::Status(code, url.to_owned()),
            other => HttpError::Transport(other.to_string()),
        })?;
        let mut buf = Vec::new();
        resp.into_reader()
            .take(2 * 1024 * 1024 * 1024) // 单响应体上限 2 GiB（防失控）
            .read_to_end(&mut buf)
            .map_err(|e| HttpError::Body(e.to_string()))?;
        Ok(buf)
    }

    /// GET 并解析 JSON 响应（packument）。
    pub fn get_json(&self, url: &str) -> Result<serde_json::Value, HttpError> {
        let bytes = self.get_bytes(url)?;
        serde_json::from_slice(&bytes).map_err(|e| HttpError::Body(format!("JSON 解析失败: {e}")))
    }
}
