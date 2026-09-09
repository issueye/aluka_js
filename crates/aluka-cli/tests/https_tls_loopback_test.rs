//! M3.2 TLS 接线验收：VM `https` 模块真实 rustls 握手（对照 Node.js 22）。
//!
//! 覆盖：
//! - https.createServer(自签 PEM) ↔ https.request 自回环（TLS 1.3 真实握手）
//! - 服务端/客户端各自独立验证：node 真 TLS 客户端连 VM server、
//!   VM 客户端连 node 真 TLS server 的跨实现对拍由 Rust 侧辅助完成
//!   （本文件 JS 探针自回环 + 跨实现见 m3_tls_loopback 之上层用例）
//! - 证书取自 tests/conformance/node22/cases/*.pem（node 与 aluka 同源）

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn alukac_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_alukac")).to_path_buf()
}

fn aluvm_exe() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_aluvm")).to_path_buf()
}

fn node_bin() -> Option<String> {
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_owned());
    if Command::new(&node)
        .arg("-v")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        Some(node)
    } else {
        None
    }
}

const PROBE: &str = r#"
// VM https 自回环：真实 TLS 1.3 握手（key/cert 与 node 探针同源 pem）
const https = require('https');
const fs = require('fs');
const pemDir = '.';
const server = https.createServer({
  key: fs.readFileSync(pemDir + '/test_key.pem'),
  cert: fs.readFileSync(pemDir + '/test_cert.pem')
}, function (req, res) {
  res.writeHead(200, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ tls: 'vm', path: req.url }));
});
server.on('error', function (e) { console.log('SERVER-ERR', e.message); });
server.listen(0, function () {
  const port = server.address().port;
  const req = https.request({ host: '127.0.0.1', port: port, rejectUnauthorized: false, path: '/vm-echo' }, function (res) {
    console.log('STATUS', res.statusCode);
    console.log('CTYPE', res.headers['content-type']);
    const chunks = [];
    res.on('data', function (c) { chunks.push(c); });
    res.on('end', function () {
      console.log('BODY', Buffer.concat(chunks).toString());
      server.close(function () { console.log('CLOSED'); });
    });
  });
  req.on('error', function (e) { console.log('REQ-ERR', String(e.message).split('\n')[0]); server.close(); });
  req.end();
});
"#;

/// M3.2 验收：VM https 自回环输出与 Node 22 逐字一致。
#[test]
fn https_self_loopback_matches_node22() {
    let work = std::env::temp_dir().join(format!("https_tls_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录");
    let pem_dir = repo_root().join("tests/conformance/node22/cases");
    std::fs::write(work.join("probe.js"), PROBE).unwrap();
    std::fs::copy(pem_dir.join("test_key.pem"), work.join("test_key.pem")).unwrap();
    std::fs::copy(pem_dir.join("test_cert.pem"), work.join("test_cert.pem")).unwrap();

    // 编译并在语料目录运行（相对 pem 路径可用）
    let bc = work.join("probe.bc");
    let build = Command::new(alukac_exe())
        .arg("compile")
        .arg(work.join("probe.js"))
        .arg("-o")
        .arg(&bc)
        .output()
        .expect("alukac");
    assert!(
        build.status.success(),
        "alukac: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(&bc)
        .current_dir(&work)
        .output()
        .expect("aluvm");
    assert!(
        out.status.success(),
        "aluvm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_owned();
    let oracle = [
        "STATUS 200",
        "CTYPE application/json",
        "BODY {\"tls\":\"vm\",\"path\":\"/vm-echo\"}",
        "CLOSED",
    ];
    let got: Vec<&str> = rust_out.lines().collect();
    assert_eq!(got, oracle, "VM https 自回环输出不符");
    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg(work.join("probe.js"))
            .current_dir(&work)
            .output()
            .expect("node");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .replace("\r\n", "\n")
            .trim()
            .to_owned();
        assert_eq!(rust_out, node_stdout, "与 Node.js 22 输出不一致");
    }
    let _ = std::fs::remove_dir_all(&work);
}
