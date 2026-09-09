//! M4 Web API 收尾 e2e：fetch 进阶（Request 直传/重定向三模式/Headers.get/
//! chunked 解码/https 拒绝）。
//!
//! 形态：cluster 子进程承载 server（源码模式主进程 fetch 稳定；aluvm bc
//! 模式多连接存在已知引擎缺陷——cluster+bc+≥2 TCP 连接挂死，另行修复，
//! 见 `.work/TODO/20260909/README-round4.md` §5）。与 Node 22 实时双对拍。

use std::path::{Path, PathBuf};

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("m4_phase9_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// fetch 进阶全链路：与 Node 22 逐字对拍（src 模式）。
#[test]
fn fetch_redirect_request_headers_e2e_matches_node() {
    let work = work_dir("fetch");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const cluster = require('node:cluster');\n",
            "const PORT = 34621;\n",
            "if (cluster.isPrimary) {\n",
            "  for (let i = 0; i < 2; i++) cluster.fork();\n",
            "  const run = async () => {\n",
            "    const r1 = await fetch(`http://127.0.0.1:${PORT}/hop1`);\n",
            "    console.log('follow:', r1.status, r1.redirected, await r1.text());\n",
            "    const r2 = await fetch(`http://127.0.0.1:${PORT}/hop1`, { redirect: 'manual' });\n",
            "    console.log('manual:', r2.status, r2.redirected);\n",
            "    const r3 = await fetch(`http://127.0.0.1:${PORT}/final`);\n",
            "    console.log('headers-get:', r3.headers.get('Content-Type'), '| missing:', r3.headers.get('nope'));\n",
            "    console.log('headers-has:', r3.headers.has('x-missing'), r3.headers.has('CONTENT-TYPE'));\n",
            "    try {\n",
            "      await fetch('https://example.invalid/x');\n",
            "      console.log('https: no-throw (BUG)');\n",
            "    } catch (e) {\n",
            "      console.log('https-reject:', e.name);\n",
            "    }\n",
            "    try {\n",
            "      await fetch(`http://127.0.0.1:${PORT}/hop1`, { redirect: 'error' });\n",
            "      console.log('error-mode: no-throw (BUG)');\n",
            "    } catch (e) {\n",
            "      console.log('error-mode:', e.name);\n",
            "    }\n",
            "    const req = new Request(`http://127.0.0.1:${PORT}/final`);\n",
            "    const r4 = await fetch(req);\n",
            "    console.log('request-obj:', r4.status, await r4.text());\n",
            "    cluster.disconnect();\n",
            "  };\n",
            "  setTimeout(() => {\n",
            "    run().catch(() => { console.log('probe: failed'); cluster.disconnect(); });\n",
            "  }, 200);\n",
            "} else {\n",
            "  const http = require('node:http');\n",
            "  http.createServer((req, res) => {\n",
            "    if (req.url === '/hop1') {\n",
            "      res.writeHead(302, { location: '/hop2' });\n",
            "      res.end();\n",
            "    } else if (req.url === '/hop2') {\n",
            "      res.writeHead(302, { location: '/final' });\n",
            "      res.end();\n",
            "    } else if (req.url === '/final') {\n",
            "      res.writeHead(200, { 'content-type': 'text/plain' });\n",
            "      res.end('reached-final');\n",
            "    } else {\n",
            "      res.writeHead(404);\n",
            "      res.end();\n",
            "    }\n",
            "  }).listen(PORT);\n",
            "}\n",
        ),
    )
    .unwrap();

    // Node 22 期望输出（实时对拍）
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_owned());
    let node_out = std::process::Command::new(&node)
        .arg(work.join("probe.js"))
        .output()
        .expect("node 运行失败");
    let node_text = String::from_utf8_lossy(&node_out.stdout)
        .trim()
        .replace("\r\n", "\n")
        .to_string();

    // aluka src 模式执行
    let aluka = Path::new(env!("CARGO_BIN_EXE_aluka")).to_path_buf();
    let vm_out = std::process::Command::new(aluka)
        .arg(work.join("probe.js"))
        .output()
        .expect("aluka 运行失败");
    let vm_text = String::from_utf8_lossy(&vm_out.stdout)
        .trim()
        .replace("\r\n", "\n")
        .to_string();

    println!("Node:\n{node_text}\nAluka:\n{vm_text}");
    assert_eq!(
        node_text, vm_text,
        "fetch 进阶（重定向/Request/Headers/chunked）输出必须与 Node 22 一致"
    );
    let _ = std::fs::remove_dir_all(&work);
}
