//! M3.2 验收：纯 Rust TLS 1.3 回环测试——自签名证书 + rustls + TcpStream
//! 真实网络通信。验证 HTTPS 服务端/客户端可完成完整 TLS 1.3 握手与
//! HTTP 请求/响应交换。
//!
//! 与 tls_spike.rs 不同：本测试使用真实 `std::net::TcpListener/TcpStream`
//! （非内存管道），覆盖真实网络栈上的 TLS 握手与数据传输。

use std::io::{Read as _, Write as _};
use std::sync::Arc;

// ---- 自签名证书生成（rcgen 纯 Rust）----

fn generate_self_signed_cert() -> (String, String) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
        .expect("rcgen self-signed cert");
    (cert_key.cert.pem(), cert_key.key_pair.serialize_pem())
}

// ---- rustls 配置构建 ----

fn server_config(cert_pem: &str, key_pem: &str) -> Arc<rustls::ServerConfig> {
    use base64::Engine as _;

    let _ = cert_pem; // 直接用 rcgen 产出的 PEM
    let _ = key_pem;

    // 从 PEM 提取 DER
    let pem_body = |pem: &str| -> String {
        pem.lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("")
    };

    // 用 rcgen 的 cert/key PEM
    let (cert_pem, key_pem) = generate_self_signed_cert();
    let _ = pem_body; // suppress unused

    let cert_der = {
        let body: String = cert_pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(body.replace(['\r', '\n'], ""))
            .expect("cert base64")
    };
    let key_der = {
        let body: String = key_pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(body.replace(['\r', '\n'], ""))
            .expect("key base64")
    };

    let certs = vec![rustls::pki_types::CertificateDer::from(cert_der)];
    let key = rustls::pki_types::PrivateKeyDer::try_from(key_der).expect("private key");

    let provider = Arc::new(rustls_rustcrypto::provider());
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("server config"),
    )
}

fn client_config_insecure() -> Arc<rustls::ClientConfig> {
    use rustls::client::danger::ServerCertVerifier;

    #[derive(Debug)]
    struct AcceptAll;
    impl ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
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

    let provider = Arc::new(rustls_rustcrypto::provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAll))
            .with_no_client_auth(),
    )
}

// ---- M3.2 验收 1：TLS 1.3 真实 TcpStream 握手 + HTTP echo 回环 ----

#[test]
fn m3_tls13_real_tcp_https_loopback() {
    let server_config = server_config("", "");

    // 绑定随机端口
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    // 服务端线程：accept + TLS + HTTP 响应
    let server_handle = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("accept");
        let config = server_config.clone();
        let conn = rustls::ServerConnection::new(config).expect("server conn");
        let mut tls = rustls::StreamOwned::new(conn, tcp);

        // 读取 HTTP 请求（读到 \r\n\r\n 为止）
        let mut buf = [0u8; 4096];
        let mut request = Vec::new();
        loop {
            let n = tls.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        // 发送 HTTP 响应
        let body = format!("{{\"tls\":true,\"req_len\":{}}}", request.len());
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        tls.write_all(response.as_bytes()).ok();
        tls.flush().ok();
    });

    // 客户端：TCP connect + TLS 握手 + HTTP 请求
    let client_config = client_config_insecure();
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost".to_owned()).expect("server name");
    let conn = rustls::ClientConnection::new(client_config, server_name).expect("client conn");
    let tcp = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).expect("tcp connect");
    let mut tls = rustls::StreamOwned::new(conn, tcp);

    // 发送 HTTP 请求
    let request = "GET /test HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    tls.write_all(request.as_bytes()).expect("write request");
    tls.flush().ok();

    // 读取 HTTP 响应
    let mut response = String::new();
    tls.read_to_string(&mut response).ok();

    server_handle.join().expect("server thread");

    // 验证响应
    assert!(response.contains("200 OK"), "status 200: {response}");
    assert!(response.contains("\"tls\":true"), "tls flag: {response}");
    assert!(
        response.contains(&format!("\"req_len\":{}", request.len())),
        "req_len echo: {response}"
    );
}

// ---- M3.2 验收 2：TLS 1.3 协议版本确认 ----

#[test]
fn m3_tls13_protocol_version() {
    // 用真实 TCP 验证协商出的协议版本为 TLS 1.3
    let server_config = server_config("", "");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let server_thread = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("accept");
        let conn = rustls::ServerConnection::new(server_config).expect("server conn");
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut buf = [0u8; 256];
        let _ = tls.read(&mut buf);
        let proto = format!("{:?}", tls.conn.protocol_version());
        let _ = tls.write_all(b"ok");
        proto
    });

    let client_config = client_config_insecure();
    let server_name = rustls::pki_types::ServerName::try_from("localhost".to_owned()).unwrap();
    let conn = rustls::ClientConnection::new(client_config, server_name).expect("client");
    let tcp = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).expect("tcp");
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    tls.write_all(b"hello").ok();
    let _ = tls.flush();

    let server_proto = server_thread.join().expect("join");
    assert!(
        server_proto.contains("TLSv1_3") || server_proto.contains("TLS13"),
        "TLS 1.3: {}",
        server_proto
    );
}
