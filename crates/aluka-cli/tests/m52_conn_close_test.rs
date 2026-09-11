//! M5.2 服务端 `Connection` 语义端到端对拍（Node.js 22 LTS 为唯一权威）。
//!
//! 覆盖登记缺口「服务端 `Connection: close`」：此前 `http/server.rs` 写出响应后
//! 只做 `mark_conn_idle`，**从不关闭**连接，且全目录无 `Connection:` 头生成。
//!
//! 权威口径（Node v22.22.2 官方实现，探针实测交叉验证）：
//! - `_http_server.js:207-209`：HTTP/1.0 请求 → `useChunkedEncodingByDefault =
//!   含 chunked 的 TE`（此处为假）、`shouldKeepAlive = false`；
//! - `_http_server.js:1073,1109`：`res.shouldKeepAlive = llhttp shouldKeepAlive`；
//! - `_http_outgoing.js:520-546`：显式 `Connection` 头则原样保留（含 `close`
//!   token → `_last`）；否则 `shouldSendKeepAlive = shouldKeepAlive &&
//!   (已设 Content-Length || useChunkedEncodingByDefault)` → 写 `keep-alive`
//!   （附 `Keep-Alive: timeout=<keepAliveTimeout/1000>`）或 `close`（`_last`）；
//! - `_http_server.js:1034-1036`：`res._last` → `socket.destroySoon()`。
//! - 副作用：HTTP/1.0 客户端既不收 `Content-Length` 也不收 chunked，响应体
//!   以关连接定界（实测确认）。
//!
//! 探针纪律：端口一律 `listen(0)` 随机取；原始报文经 `net` 客户端捕获后按字段
//! 归一（`Date` 剔除、`content-type` 属既有 Go 风格偏差故从顺序对比剔除）；
//! 复用的第二个请求在「首个响应到达」时发出，不用定时器排序——aluka 的定时器
//! 到期时间为「累加到队尾」模型，多定时器并存时触发顺序与 Node 不同，会污染判定。

mod common;

fn run_probe(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("m52_conn_close_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_node(&work, "probe.js")
}

/// 五情形原始报文对拍：请求 `close` / 响应设 `close` / HTTP/1.0（无 keep-alive）
/// / HTTP/1.0 + keep-alive（仍为 close）/ 默认（keep-alive + `Keep-Alive: timeout=5`），
/// 并断言服务端是否真的发出 FIN。
#[test]
fn http_connection_close_semantics_match_node() {
    let out = run_probe(
        "semantics",
        r#"
const http = require("http");
const net = require("net");

const SCENARIOS = [
  {
    label: "req-close",
    setup: function () {},
    req: "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
  },
  {
    label: "res-close",
    setup: function (res) { res.setHeader("Connection", "close"); },
    req: "GET / HTTP/1.1\r\nHost: x\r\n\r\n",
  },
  {
    label: "http10",
    setup: function () {},
    req: "GET / HTTP/1.0\r\nHost: x\r\n\r\n",
  },
  {
    label: "http10-keepalive",
    setup: function () {},
    req: "GET / HTTP/1.0\r\nHost: x\r\nConnection: keep-alive\r\n\r\n",
  },
  {
    label: "default",
    setup: function () {},
    req: "GET / HTTP/1.1\r\nHost: x\r\n\r\n",
  },
];

function report(label, raw, ended, closed) {
  var idx = raw.indexOf("\r\n\r\n");
  var headText = idx >= 0 ? raw.slice(0, idx) : raw;
  var lines = headText.split("\r\n");
  var hdrs = {};
  for (var i = 1; i < lines.length; i++) {
    var c = lines[i].indexOf(":");
    if (c < 0) continue;
    var k = lines[i].slice(0, c).trim().toLowerCase();
    if (k === "date") continue;
    hdrs[k] = lines[i].slice(c + 1).trim();
  }
  var body = idx >= 0 ? raw.slice(idx + 4) : "";
  function show(name) {
    return hdrs[name] === undefined ? "<absent>" : hdrs[name];
  }
  console.log("[" + label + "]");
  console.log("  status-line=" + lines[0]);
  console.log("  connection=" + show("connection"));
  console.log("  keep-alive=" + show("keep-alive"));
  console.log("  content-length=" + show("content-length"));
  console.log("  transfer-encoding=" + show("transfer-encoding"));
  console.log("  body=" + JSON.stringify(body));
  var scoped = ["connection", "keep-alive", "content-length", "transfer-encoding"];
  console.log("  order-scoped=" + lines.slice(1).map(function (l) {
    return l.slice(0, l.indexOf(":")).toLowerCase();
  }).filter(function (n) {
    return scoped.indexOf(n) >= 0;
  }).join(","));
  console.log("  server-ended=" + ended);
  console.log("  server-closed=" + closed);
}

var i = 0;
function runNext() {
  if (i >= SCENARIOS.length) return;
  var sc = SCENARIOS[i++];
  var server = http.createServer(function (req, res) {
    sc.setup(res);
    res.end("hello");
  });
  server.listen(0, "127.0.0.1", function () {
    var port = server.address().port;
    var sock = net.connect(port, "127.0.0.1");
    var raw = "";
    var ended = false;
    var closed = false;
    sock.setEncoding("latin1");
    sock.on("connect", function () { sock.write(sc.req); });
    sock.on("data", function (d) { raw += d; });
    sock.on("end", function () { ended = true; });
    sock.on("close", function () { closed = true; });
    sock.on("error", function () {});
    setTimeout(function () {
      report(sc.label, raw, ended, closed);
      sock.destroy();
      server.close();
      setTimeout(runNext, 50);
    }, 500);
  });
}

runNext();
"#,
    );
    // 逐字段断言（防止「两侧同为空白输出」式的假一致）。
    let expected = "\
[req-close]
  status-line=HTTP/1.1 200 OK
  connection=close
  keep-alive=<absent>
  content-length=5
  transfer-encoding=<absent>
  body=\"hello\"
  order-scoped=connection,content-length
  server-ended=true
  server-closed=true
[res-close]
  status-line=HTTP/1.1 200 OK
  connection=close
  keep-alive=<absent>
  content-length=5
  transfer-encoding=<absent>
  body=\"hello\"
  order-scoped=connection,content-length
  server-ended=true
  server-closed=true
[http10]
  status-line=HTTP/1.1 200 OK
  connection=close
  keep-alive=<absent>
  content-length=<absent>
  transfer-encoding=<absent>
  body=\"hello\"
  order-scoped=connection
  server-ended=true
  server-closed=true
[http10-keepalive]
  status-line=HTTP/1.1 200 OK
  connection=close
  keep-alive=<absent>
  content-length=<absent>
  transfer-encoding=<absent>
  body=\"hello\"
  order-scoped=connection
  server-ended=true
  server-closed=true
[default]
  status-line=HTTP/1.1 200 OK
  connection=keep-alive
  keep-alive=timeout=5
  content-length=5
  transfer-encoding=<absent>
  body=\"hello\"
  order-scoped=connection,keep-alive,content-length
  server-ended=false
  server-closed=false";
    assert_eq!(out.trim(), expected);
}

/// 复用对拍：`Connection: keep-alive` 下同一 socket 串行两次请求都应得到响应、
/// 服务端不关连接；`Connection: close` 下第二次请求不再得到响应且服务端已关闭。
#[test]
fn http_connection_reuse_matches_node() {
    let out = run_probe(
        "reuse",
        r#"
const http = require("http");
const net = require("net");

function count(s, sub) {
  var n = 0, i = 0;
  while ((i = s.indexOf(sub, i)) >= 0) { n++; i += sub.length; }
  return n;
}

var CASES = [
  { label: "keepalive-reuse", conn: "Connection: keep-alive\r\n" },
  { label: "req-close-no-reuse", conn: "Connection: close\r\n" },
];

function request(path, conn) {
  return "GET " + path + " HTTP/1.1\r\nHost: x\r\n" + conn + "\r\n";
}

var i = 0;
function runNext() {
  if (i >= CASES.length) return;
  var c = CASES[i++];
  var server = http.createServer(function (req, res) { res.end("hello"); });
  server.listen(0, "127.0.0.1", function () {
    var port = server.address().port;
    var sock = net.connect(port, "127.0.0.1");
    var raw = "";
    var ended = false;
    var sentSecond = false;
    sock.setEncoding("latin1");
    sock.on("connect", function () {
      sock.write(request("/1", c.conn));
    });
    sock.on("data", function (d) {
      raw += d;
      if (!sentSecond && raw.indexOf("hello") >= 0) {
        sentSecond = true;
        sock.write(request("/2", c.conn));
      }
    });
    sock.on("end", function () { ended = true; });
    sock.on("close", function () { ended = true; });
    sock.on("error", function () {});
    setTimeout(function () {
      console.log("[" + c.label + "]");
      console.log("  resp-count=" + count(raw, "HTTP/1.1 200 OK"));
      console.log("  sent-second=" + sentSecond);
      console.log("  server-ended=" + ended);
      sock.destroy();
      server.close();
      setTimeout(runNext, 50);
    }, 700);
  });
}

runNext();
"#,
    );
    let expected = "\
[keepalive-reuse]
  resp-count=2
  sent-second=true
  server-ended=false
[req-close-no-reuse]
  resp-count=1
  sent-second=true
  server-ended=true";
    assert_eq!(out.trim(), expected);
}
