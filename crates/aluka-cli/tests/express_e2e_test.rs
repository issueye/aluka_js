//! M2.4 Express 真实依赖树端到端：6 大核心场景与 Node.js 22 oracle 对拍。
//!
//! **fixture 自包含（M5 评审修复）**：app.js 源码固化在测试内写入临时
//! 目录，依赖 `demo/express-demo/node_modules`（express@4 全树，`npm ci`
//! 安装、不入库）——`node_modules` 缺失时跳过（CI 不阻塞门禁）。
//! 旧实现依赖仓库根的 demo/express-demo/app.js（该文件在 .gitignore、
//! 且会被开发者本地改动），固化 oracle 随环境漂移导致门禁间歇失败。
//!
//! 流程：写入 fixture → `alukac build` 全树预编译 → `aluvm run` 执行 →
//! stdout 与固化 oracle 行逐条断言，并（Node 可用时）与 `node app.js`
//! 实时对拍。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 仓库根：`crates/aluka-cli` 上溯两级（aluka-cli → crates → 仓库根）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn node_modules_dir() -> PathBuf {
    repo_root().join("demo").join("express-demo")
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

/// fixture 就绪：express 已安装才跑（npm ci 后 68 包）。
fn fixture_ready() -> bool {
    node_modules_dir()
        .join("node_modules")
        .join("express")
        .is_dir()
}

/// 固化 fixture：Express 6 场景（GET / · echo/:word · POST /json · 并发 ·
/// 自定义 Content-Type · 优雅退出）。Node 与 aluvm 共用、逐字对拍。
const APP_JS: &str = r#"
var express = require('express');
var http = require('http');

var app = express();
app.use(express.json());

app.get('/', function (req, res) { res.send('hello from express'); });
app.get('/echo/:word', function (req, res) { res.send('echo: ' + req.params.word); });
app.post('/json', function (req, res) { res.json({ got: req.body }); });
app.get('/ctype', function (req, res) {
  res.type('application/xml');
  res.send('<root>ok</root>');
});

function httpReq(port, path, method, body) {
  return new Promise(function (resolve, reject) {
    var payload = body === undefined ? null : JSON.stringify(body);
    var req = http.request({
      port: port, path: path, method: method || 'GET',
      headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {}
    }, function (res) {
      var chunks = [];
      res.on('data', function (c) { chunks.push(c); });
      res.on('end', function () {
        var text = Buffer.concat(chunks).toString('utf8');
        resolve({ status: res.statusCode, ctype: res.headers['content-type'] || '', body: text });
      });
    });
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

var server = app.listen(0, function () {
  var port = server.address().port;
  console.log('PORT_READY');
  (async function () {
    try {
      var r1 = await httpReq(port, '/', 'GET');
      console.log('GET / ->', r1.status, r1.body);
      var r2 = await httpReq(port, '/echo/world', 'GET');
      console.log('ECHO ->', r2.status, r2.body);
      var r3 = await httpReq(port, '/json', 'POST', { n: 1 });
      console.log('POST ->', r3.status, r3.body);
      var rs = await Promise.all([
        httpReq(port, '/echo/a', 'GET'),
        httpReq(port, '/echo/b', 'GET'),
        httpReq(port, '/echo/c', 'GET')
      ]);
      console.log('CONCURRENT ->', rs.map(function (r) { return r.status; }).join(','));
      var r4 = await httpReq(port, '/ctype', 'GET');
      console.log('CTYPE ->', r4.status, r4.ctype, '|', r4.body);
      server.close(function () { console.log('CLOSED'); });
    } catch (e) {
      console.log('SCENARIO FAIL:', e && e.message ? e.message : String(e));
      process.exit(1);
    }
  })();
});
"#;

/// 固化 oracle（Node 22 实测，逐行断言防漂移）。
const ORACLE_LINES: &[&str] = &[
    "PORT_READY",
    "GET / -> 200 hello from express",
    "ECHO -> 200 echo: world",
    "POST -> 200 {\"got\":{\"n\":1}}",
    "CONCURRENT -> 200,200,200",
    "CTYPE -> 200 application/xml; charset=utf-8 | <root>ok</root>",
    "CLOSED",
];

/// M2.4 验收：Express 真实依赖树 6 大核心场景与 Node 22 对拍。
#[test]
fn express_six_scenes_match_node22_oracle() {
    if !fixture_ready() {
        eprintln!("skip: demo/express-demo/node_modules 缺失（需 `npm ci`，fixture 不入库）");
        return;
    }

    // 0. fixture 落盘：app.js 以唯一名直接放在 demo/express-demo 根
    // （`.e2e_app_<pid>.js`，测试后清理）。build 的 root = app.js 所在
    // 目录 = demo/express-demo——与既有 demo 布局一致，node_modules 依赖
    // 全部镜像进 aluka_build/node_modules（旧实现把 fixture 放在临时目录，
    // root 之外依赖退化 _ext/ 导致运行期 Cannot find module）。
    let work = node_modules_dir();
    let app_name = format!(".e2e_app_{}.js", std::process::id());
    let app_path = work.join(&app_name);
    let _ = std::fs::remove_file(&app_path);
    std::fs::write(&app_path, APP_JS).expect("写入 fixture 失败");

    // 1. 全树预编译（入口的 require 闭包，132 模块级）
    let build = Command::new(alukac_exe())
        .arg("build")
        .arg(&app_name)
        .current_dir(&work)
        .output()
        .expect("alukac build 失败");
    assert!(
        build.status.success(),
        "alukac build 失败: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // 2. aluvm 执行入口字节码
    let out = Command::new(aluvm_exe())
        .arg("run")
        .arg(
            work.join("aluka_build")
                .join(format!(".e2e_app_{}.bc", std::process::id())),
        )
        .current_dir(&work)
        .output()
        .expect("aluvm 运行失败");
    assert!(
        out.status.success(),
        "aluvm 运行失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust_out = String::from_utf8_lossy(&out.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_owned();

    // 3. 固化 oracle 逐行断言（6 场景全绿）
    let got: Vec<&str> = rust_out.lines().collect();
    assert_eq!(
        got, ORACLE_LINES,
        "Express 6 场景输出必须与 Node 22 oracle 逐行一致"
    );

    // 4. Node 22 实时对拍（权威 oracle 在场时）
    if let Some(node) = node_bin() {
        let node_out = Command::new(node)
            .arg(&app_path)
            .current_dir(&work)
            .output()
            .expect("运行 Node.js 失败");
        let node_stdout = String::from_utf8_lossy(&node_out.stdout)
            .replace("\r\n", "\n")
            .trim()
            .to_owned();
        println!("Aluka:\n{rust_out}\nNode:\n{node_stdout}");
        assert_eq!(rust_out, node_stdout, "Express 输出必须与 Node.js 22 一致");
    }
    // 5. 清理 fixture（源文件与编译产物；`aluka_build/` 既有内容不动）
    let _ = std::fs::remove_file(&app_path);
    let _ = std::fs::remove_file(
        work.join("aluka_build")
            .join(format!(".e2e_app_{}.bc", std::process::id())),
    );
}
