//! `node:tls` 内置模块（Phase 5）：TLS/SSL 网络（API 表面对齐）。
//!
//! 与 Node.js 22 LTS 标准逐字对齐的部分：
//! - `tls.createServer([options][, secureConnectionListener])`：选项校验错误消息：
//!   * 无选项 → `tls: createServer requires { key, cert } options`；
//!   * 缺 key/cert → `tls: createServer requires { key, cert } PEM options`；
//!   * PEM 解析失败 → `tls: invalid key/cert: tls: failed to find any PEM
//!     data in certificate input`（证书） / `...in key input`（私钥）；
//! - `tls.createSecureContext([options])`：key/cert 均为可识别 PEM 时设置
//!   `key`/`cert` 属性，恒含 `context` 对象；
//! - `tls.checkServerIdentity(hostname, cert)` → `undefined`；
//! - `tls.getCiphers()`：标准 TLS 套件名同序的 13 个套件名；
//! - `tls.connect(options[, connectListener])` / `tls.TLSSocket`：socket 表面
//!   （on/write/end/destroy 等，复用 `net:socket` 命名空间）。
//!
//! **已知限制（刻意设计）**：纯 Rust 约束下（禁 C 依赖）不实现真实 TLS
//! 握手路径——`createServer` 不绑定真实端口、`connect` 不发起真实连接，
//! 仅保证 API 表面与选项校验语义；对拍探针绝不触发真实握手。
//! PEM 校验为标记级检查（`-----BEGIN` 块存在性），完整 X.509 解析未实现。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("tls")` / `require("node:tls")` 模块定义。
pub const MODULE: ModuleDef = ModuleDef { name: "tls", build };

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    for export in [
        "createServer",
        "connect",
        "TLSSocket",
        "createSecureContext",
        "checkServerIdentity",
        "getCiphers",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("tls.{export}"));
        set_module_prop(vm, obj, export, Value::Object(fn_ref))?;
    }

    register_handler(registry, "tls", "createServer", tls_create_server);
    register_handler(registry, "tls", "connect", tls_connect);
    register_handler(registry, "tls", "TLSSocket", tls_socket_ctor);
    register_handler(
        registry,
        "tls",
        "createSecureContext",
        tls_create_secure_context,
    );
    register_handler(
        registry,
        "tls",
        "checkServerIdentity",
        tls_check_server_identity,
    );
    register_handler(registry, "tls", "getCiphers", tls_get_ciphers);

    Ok(obj)
}

/// 从 options 值提取 (key, cert) PEM 字符串（对齐 Go TLSConfigFromOptions 读取）。
fn extract_pem_options(vm: &mut Vm, options: Option<Value>) -> (String, String) {
    let mut key = String::new();
    let mut cert = String::new();
    if let Some(opts) = options {
        if let Ok(v) = vm.get_property(opts, "key") {
            if !matches!(v, Value::Undefined) {
                key = vm.format_value(v);
            }
        }
        if let Ok(v) = vm.get_property(opts, "cert") {
            if !matches!(v, Value::Undefined) {
                cert = vm.format_value(v);
            }
        }
    }
    (key, cert)
}

/// PEM 解析校验（标记级）：返回 Go X509KeyPair 对应形态的错误消息。
/// 证书缺 CERTIFICATE 块 → `...in certificate input`；私钥缺 BEGIN 块 →
/// `...in key input`；两者齐全 → None。
fn validate_pem(key: &str, cert: &str) -> Option<String> {
    if !cert.contains("-----BEGIN CERTIFICATE-----") {
        return Some(
            "tls: invalid key/cert: tls: failed to find any PEM data in certificate input"
                .to_owned(),
        );
    }
    if !key.contains("-----BEGIN") {
        return Some(
            "tls: invalid key/cert: tls: failed to find any PEM data in key input".to_owned(),
        );
    }
    None
}

/// `tls.createServer([options][, secureConnectionListener])`。
///
/// 选项校验与 Go 逐字一致；校验通过后返回 server 表面对象。
/// **TLS 握手路径未实现**：`listen` 仅派发 'listening' 事件与回调，
/// 不绑定真实端口（探针绝不触发真实握手）。
fn tls_create_server(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut options: Option<Value> = None;
    // secureConnectionListener 同样被扫描（Go 语义），但握手路径未实现故不接线。
    for a in args {
        if crate::builtins::dns_promises::is_plain_object(vm, *a) {
            options = Some(*a);
        }
    }
    // 对齐 Go TLSConfigFromOptions 的三层错误消息。
    let Some(opts) = options.filter(|v| !v.is_undefined()) else {
        let err = vm.alloc_error_instance("tls: createServer requires { key, cert } options");
        return Err(VmError::Thrown(Value::Object(err)));
    };
    let (key, cert) = extract_pem_options(vm, Some(opts));
    if key.is_empty() || cert.is_empty() {
        let err = vm.alloc_error_instance("tls: createServer requires { key, cert } PEM options");
        return Err(VmError::Thrown(Value::Object(err)));
    }
    if let Some(message) = validate_pem(&key, &cert) {
        let err = vm.alloc_error_instance(&message);
        return Err(VmError::Thrown(Value::Object(err)));
    }

    // server 表面（复用 net:server 命名空间的事件与工具方法）。
    let server = vm.alloc_ordinary();
    let ns = vm.alloc_string("net:server".to_owned());
    let _ = vm.set_property(Value::Object(server), "_builtinNs", Value::Object(ns));
    for method in [
        "listen",
        "close",
        "address",
        "getConnections",
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "listenerCount",
        "ref",
        "unref",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("net:server.{method}"));
        let _ = vm.set_property(Value::Object(server), method, Value::Object(fn_ref));
    }
    let _ = vm.set_property(Value::Object(server), "listening", Value::Boolean(false));
    let _ = vm.set_property(Value::Object(server), "maxConnections", Value::Number(0.0));
    Ok(Value::Object(server))
}

/// `tls.connect(options[, connectListener])`。
///
/// **TLS 握手路径未实现**：返回未连接的 TLSSocket 表面对象，不发起真实
/// 连接（对齐 Go 的返回形态；探针绝不触发真实握手）。
fn tls_connect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let socket = create_tls_socket(vm);
    let _ = args;
    Ok(Value::Object(socket))
}

/// `new tls.TLSSocket()`：未连接的 TLSSocket 表面（destroy 可释放）。
fn tls_socket_ctor(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let socket = create_tls_socket(vm);
    Ok(Value::Object(socket))
}

/// 创建 TLSSocket 表面对象（复用 net:socket 命名空间全套方法）。
fn create_tls_socket(vm: &mut Vm) -> ObjectRef {
    let obj = vm.alloc_ordinary();
    let ns = vm.alloc_string("net:socket".to_owned());
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", Value::Object(ns));
    for method in [
        "write",
        "end",
        "destroy",
        "on",
        "addListener",
        "once",
        "emit",
        "off",
        "removeListener",
        "listenerCount",
        "address",
        "pipe",
        "setEncoding",
        "setNoDelay",
        "setTimeout",
        "setKeepAlive",
        "ref",
        "unref",
        "pause",
        "resume",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("net:socket.{method}"));
        let _ = vm.set_property(Value::Object(obj), method, Value::Object(fn_ref));
    }
    let _ = vm.set_property(Value::Object(obj), "bytesRead", Value::Number(0.0));
    obj
}

/// `tls.createSecureContext([options])`：key/cert 均为可识别 PEM 时设置属性，
/// 恒含 `context`（对齐 Go：X509KeyPair 失败则不设置 key/cert）。
fn tls_create_secure_context(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let sc = vm.alloc_ordinary();
    let options = args.first().copied();
    let (key, cert) = extract_pem_options(vm, options);
    if !key.is_empty() && !cert.is_empty() && validate_pem(&key, &cert).is_none() {
        let s_alloc0 = vm.alloc_string(key);
        let _ = vm.set_property(Value::Object(sc), "key", Value::Object(s_alloc0));
        let s_alloc0 = vm.alloc_string(cert);
        let _ = vm.set_property(Value::Object(sc), "cert", Value::Object(s_alloc0));
    }
    let ctx = Value::Object(vm.alloc_ordinary());
    let _ = vm.set_property(Value::Object(sc), "context", ctx);
    Ok(Value::Object(sc))
}

/// `tls.checkServerIdentity(hostname, cert)`：简化恒 `undefined`（对齐 Go）。
fn tls_check_server_identity(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Undefined)
}

/// `tls.getCiphers()`：标准 TLS 套件名（硬编码常量列表）。
fn tls_get_ciphers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let vals: Vec<Value> = STANDARD_CIPHER_SUITES
        .iter()
        .map(|name| Value::Object(vm.alloc_string((*name).to_owned())))
        .collect();
    Ok(Value::Object(vm.alloc_array(vals)))
}

/// 标准 TLS 套件名列表（对齐 Node.js 22 LTS 标准输出，同序）。
const STANDARD_CIPHER_SUITES: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
];

// ---- M3.2 rustls 真实 TLS 层（https 服务端/客户端接线；net 侧后续复用）----

use std::sync::Arc;

#[allow(dead_code)]
/// TLS 测试证书（EC P-256 CN=localhost 自签名，与 tls_spike.rs 同套）。
pub(crate) const TEST_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----MIIBfTCCASOgAwIBAgIUPVhrgGjBtukok6rwdJylvYl2UKEwCgYIKoZIzj0EAwIwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDkwNTAzNDg0NloXDTM2MDkwMjAzNDg0NlowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEFFzrytgxmKNhGAaOJYcZRzZ2poc8ZYnrfPFwcCapXw6NFQpIA/GQHMtMV49mcvyJr2XhbmZpOsR88cEXBR7tOaNTMFEwHQYDVR0OBBYEFLHI9r+4mBfu/6X1ND5FdhyqbGFXMB8GA1UdIwQYMBaAFLHI9r+4mBfu/6X1ND5FdhyqbGFXMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIgaNRObbGR5CinTtXxx3RCAJIHpTcGGqXXT3M8Dxgs4fwCIQCXqnOaJfB8AH6G3WIRullF5mzYpg5OU8ViVMtxuSdhzw==-----END CERTIFICATE-----";

/// TLS 测试私钥（PKCS#8 EC P-256，与上 cert 配对）。
#[allow(dead_code)]
pub(crate) const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgEfccIWANmwviC3OWEfx26Fe479xclCh3ZDCDX0g4vKihRANCAAQUXOvK2DGYo2EYBo4lhxlHNnamhzxliet88XBwJqlfDo0VCkgD8ZAcy0xXj2Zy/ImvZeFuZmk6xHzxwRcFHu05-----END PRIVATE KEY-----";

/// rustls 全局加密提供者（懒初始化一次）。
#[allow(dead_code)]
pub(crate) fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    static P: std::sync::OnceLock<Arc<rustls::crypto::CryptoProvider>> = std::sync::OnceLock::new();
    P.get_or_init(|| Arc::new(rustls_rustcrypto::provider()))
        .clone()
}

/// PEM → DER 载荷（剥离 PEM 包装，base64 解码）。
#[allow(dead_code)]
pub(crate) fn pem_to_der(pem: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("PEM base64 decode: {e}"))
}

/// 从 PEM 密钥/证书构建 rustls ServerConfig。
#[allow(dead_code)]
pub(crate) fn make_server_config(
    key_pem: &str,
    cert_pem: &str,
) -> Result<rustls::ServerConfig, String> {
    let cert_der = pem_to_der(cert_pem)?;
    let certs = vec![rustls::pki_types::CertificateDer::from_slice(&cert_der).into_owned()];
    let key_der = pem_to_der(key_pem)?;
    let key = rustls::pki_types::PrivateKeyDer::try_from(key_der)
        .map_err(|e| format!("private key: {e}"))?;
    let mut cfg = rustls::ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("protocol versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("server cert: {e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(cfg)
}

/// 接收任意服务端证书的客户端校验器（对齐 tls_spike 评估；证书链校验为
/// 后续工作项——Node 对拍探针以 `rejectUnauthorized: false` 运行）。
#[derive(Debug)]
struct AcceptAllVerifier;

impl rustls::client::danger::ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

/// 构建 HTTPS 客户端 rustls 配置（ALPN http/1.1；证书校验接收任意）。
pub(crate) fn make_client_config() -> Result<rustls::ClientConfig, String> {
    let mut cfg = rustls::ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("protocol versions: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAllVerifier))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(cfg)
}

/// 由主机名/地址构造 rustls ServerName。
pub(crate) fn server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, String> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        Ok(rustls::pki_types::ServerName::IpAddress(ip.into()))
    } else {
        rustls::pki_types::ServerName::try_from(host.to_owned())
            .map_err(|e| format!("server name: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_suites_match_standard_list() {
        assert_eq!(STANDARD_CIPHER_SUITES.len(), 13);
        assert_eq!(STANDARD_CIPHER_SUITES[0], "TLS_AES_128_GCM_SHA256");
        assert_eq!(
            STANDARD_CIPHER_SUITES[12],
            "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256"
        );
    }

    #[test]
    fn pem_validation_mirrors_go_messages() {
        let cert_only = "-----BEGIN CERTIFICATE-----\nxxx\n-----END CERTIFICATE-----\n";
        assert_eq!(
            validate_pem("k", "c").as_deref(),
            Some("tls: invalid key/cert: tls: failed to find any PEM data in certificate input")
        );
        assert_eq!(
            validate_pem("k", cert_only).as_deref(),
            Some("tls: invalid key/cert: tls: failed to find any PEM data in key input")
        );
        assert_eq!(
            validate_pem(
                "-----BEGIN PRIVATE KEY-----\nk\n-----END PRIVATE KEY-----\n",
                cert_only
            ),
            None
        );
    }
}
