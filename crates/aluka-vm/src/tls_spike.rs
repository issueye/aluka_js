//! TLS spike（T10 评估）：rustls + rustls-rustcrypto 纯 Rust 提供商的
//! TLS 握手与数据面验证（仅测试编译，不进生产路径）。
//!
//! 评估结论（详见 `.work/evidence/20260905/tier3-report.md`）：
//! - 依赖树纯 Rust（rustls 路径无 ring / aws-lc-rs / C）；
//! - 完整 TLS 1.3 握手 + 双向数据在内存管道跑通——**纯 Rust 真实 TLS 可行**；
//! - JS 模块接线（https.createServer/request 经事件泵挂 rustls 会话）为
//!   下一步，设计草图见证据报告。

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

/// 单端管道：本地收包缓冲 + 对端写入句柄 + 对端关闭标志。
struct PipeEnd {
    inbound: Rc<RefCell<VecDeque<u8>>>,
    peer_inbound: Rc<RefCell<VecDeque<u8>>>,
    peer_closed: Rc<std::cell::Cell<bool>>,
}

impl PipeEnd {
    /// 建立一对互连管道端。
    fn pair() -> (PipeEnd, PipeEnd) {
        let a_in = Rc::new(RefCell::new(VecDeque::new()));
        let b_in = Rc::new(RefCell::new(VecDeque::new()));
        let a_closed = Rc::new(std::cell::Cell::new(false));
        let b_closed = Rc::new(std::cell::Cell::new(false));
        (
            PipeEnd {
                inbound: a_in.clone(),
                peer_inbound: b_in.clone(),
                peer_closed: b_closed.clone(),
            },
            PipeEnd {
                inbound: b_in,
                peer_inbound: a_in,
                peer_closed: a_closed,
            },
        )
    }
}

impl std::io::Read for PipeEnd {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut side = self.inbound.borrow_mut();
        let n = buf.len().min(side.len());
        for slot in buf.iter_mut().take(n) {
            *slot = side.pop_front().unwrap();
        }
        if n == 0 {
            if self.peer_closed.get() {
                return Ok(0); // EOF
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "pipe empty",
            ));
        }
        Ok(n)
    }
}

impl std::io::Write for PipeEnd {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.peer_closed.get() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "peer closed",
            ));
        }
        self.peer_inbound.borrow_mut().extend(buf.iter().copied());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 客户端证书校验放行（spike 测试用；生产实现须接真实校验链）。
#[derive(Debug)]
struct AcceptAll;

impl rustls::client::danger::ServerCertVerifier for AcceptAll {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
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

/// 测试证书（openssl EC P-256 自签，CN=localhost，生成命令见证据报告）。
const TEST_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBfTCCASOgAwIBAgIUPVhrgGjBtukok6rwdJylvYl2UKEwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDkwNTAzNDg0NloXDTM2MDkwMjAz
NDg0NlowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAEFFzrytgxmKNhGAaOJYcZRzZ2poc8ZYnrfPFwcCapXw6NFQpIA/GQHMtM
V49mcvyJr2XhbmZpOsR88cEXBR7tOaNTMFEwHQYDVR0OBBYEFLHI9r+4mBfu/6X1
ND5FdhyqbGFXMB8GA1UdIwQYMBaAFLHI9r+4mBfu/6X1ND5FdhyqbGFXMA8GA1Ud
EwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIgaNRObbGR5CinTtXxx3RCAJIH
pTcGGqXXT3M8Dxgs4fwCIQCXqnOaJfB8AH6G3WIRullF5mzYpg5OU8ViVMtxuSdh
zw==
-----END CERTIFICATE-----";

/// 测试私钥（PKCS#8 EC P-256，与上 cert 配对）。
const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgEfccIWANmwviC3OW
Efx26Fe479xclCh3ZDCDX0g4vKihRANCAAQUXOvK2DGYo2EYBo4lhxlHNnamhzxl
iet88XBwJqlfDo0VCkgD8ZAcy0xXj2Zy/ImvZeFuZmk6xHzxwRcFHu05
-----END PRIVATE KEY-----";

/// 驱动两侧 io：write_tls / read_tls / process_new_packets 显式泵（无进展即停）。
fn pump(
    server: &mut rustls::ServerConnection,
    server_io: &mut PipeEnd,
    client: &mut rustls::ClientConnection,
    client_io: &mut PipeEnd,
) {
    for _ in 0..200 {
        let mut progressed = false;
        // 出站：两侧各自的 TLS 记录写入线路
        match server.write_tls(server_io) {
            Ok(n) if n > 0 => progressed = true,
            _ => {}
        }
        match client.write_tls(client_io) {
            Ok(n) if n > 0 => progressed = true,
            _ => {}
        }
        // 入站：读取并处理对端记录
        match server.read_tls(server_io) {
            Ok(n) if n > 0 => {
                let _state = server.process_new_packets();
                progressed = true;
            }
            _ => {}
        }
        match client.read_tls(client_io) {
            Ok(n) if n > 0 => {
                let _ = client.process_new_packets();
                progressed = true;
            }
            _ => {}
        }
        if !progressed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    /// 纯 Rust TLS：完整握手 + echo 往返（rustls-rustcrypto 提供商）。
    #[test]
    fn tls13_handshake_and_echo_over_pure_rust_provider() {
        let provider = Arc::new(rustls_rustcrypto::provider());
        use base64::Engine as _;
        let b64_body = |pem: &str| -> String {
            pem.lines()
                .filter(|l| !l.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("")
        };
        let cert_der = base64::engine::general_purpose::STANDARD
            .decode(b64_body(TEST_CERT_PEM))
            .expect("测试证书 base64");
        let certs = vec![CertificateDer::from_slice(&cert_der).into_owned()];
        let key_der = base64::engine::general_purpose::STANDARD
            .decode(b64_body(TEST_KEY_PEM))
            .expect("测试私钥 base64");
        let key = PrivateKeyDer::try_from(key_der).expect("测试私钥");

        let mut server_config = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("协议版本")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("服务端证书装配");
        server_config.alpn_protocols = vec![b"http/1.1".to_vec()];

        let mut client_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("协议版本")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAll))
            .with_no_client_auth();

        let (mut server_io, mut client_io) = PipeEnd::pair();
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost".to_owned()).expect("server name");
        let mut server =
            rustls::ServerConnection::new(Arc::new(server_config)).expect("server session");
        let _ = &mut client_config;
        let mut client = rustls::ClientConnection::new(Arc::new(client_config), server_name)
            .expect("client session");

        pump(&mut server, &mut server_io, &mut client, &mut client_io);
        assert!(
            !server.is_handshaking() && !client.is_handshaking(),
            "TLS 1.3 握手必须完成"
        );

        // echo 往返：client → server → client
        client.writer().write_all(b"ping").unwrap();
        pump(&mut server, &mut server_io, &mut client, &mut client_io);
        let mut buf = [0u8; 16];
        let n = server.reader().read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        server.writer().write_all(b"pong").unwrap();
        pump(&mut server, &mut server_io, &mut client, &mut client_io);
        let n = client.reader().read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"pong");
    }
}
