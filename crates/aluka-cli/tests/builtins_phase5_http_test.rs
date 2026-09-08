//! Phase 5 http / https / http2 家族内置库端到端与接口对拍测试：
//! - `http`：server+client GET 往返（状态码/body/头）、POST body 回显、
//!   ServerResponse.writeHead（头冻结/204 无 CL）、生命周期事件
//!   （listen callback → 'listening' → 'connection'）、STATUS_CODES/METHODS、
//!   Agent/globalAgent/构造器表面、无 handler 500 兜底、字符串 URL；
//! - `https`：{key, cert} 缺失/无效的错误消息逐字对齐、模块表面、
//!   `Server` 构造返回对象；
//! - `http2`：constants/getDefaultSettings/getPackedSettings/sensitiveHeaders
//!   表面、createSecureServer 校验错误、connect 会话表面与 'connect' 事件。
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针纪律：端口固定使用 47140-47169（net 组占用 47110-47139），
//! 全部 127.0.0.1；响应头只打印确定字段（Date 值不打印，仅测存在性）。
//! 注：Rust VM 暂无 `Object.keys`/`String()` 全局，探针一律用 `for-in`
//! 与字符串拼接等价表达（Go 侧输出一致）。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase5_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 http server + client GET 往返：状态码/statusMessage/响应头选中项
/// 与 body 逐字对齐（含服务端自动补的 content-type/content-length）。
#[test]
fn http_get_roundtrip_matches_go() {
    let work = work_dir("get_roundtrip");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const q = (v) => JSON.stringify(v) === undefined ? '\"' + v + '\"' : JSON.stringify(v);\n",
            "const server = http.createServer((req, res) => {\n",
            "    console.log(\"req:\", req.method, req.url, req.httpVersion, q(req.headers[\"user-agent\"]));\n",
            "    res.end(\"hello\");\n",
            "});\n",
            "server.listen(47141, \"127.0.0.1\", () => {\n",
            "    const req = http.get({ host: \"127.0.0.1\", port: 47141, path: \"/abc\" }, (res) => {\n",
            "        console.log(\"status:\", res.statusCode, q(res.statusMessage));\n",
            "        console.log(\"ctype:\", res.headers[\"content-type\"], \"clen:\", res.headers[\"content-length\"]);\n",
            "        console.log(\"date:\", typeof res.headers[\"date\"], \"trailers:\", typeof res.trailers);\n",
            "        let body = \"\";\n",
            "        res.on(\"data\", (c) => { body += c.toString(); });\n",
            "        res.on(\"end\", () => { console.log(\"body:\", body); server.close(); });\n",
            "    });\n",
            "    console.log(\"req surface:\", typeof req, typeof req.write, typeof req.end, typeof req.abort);\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("status: 200 \"200 OK\"")
            && out.contains("ctype: text/plain; charset=utf-8 clen: 5")
            && out.contains("body: hello")
            && out.contains("date: string"),
        "GET 往返输出不符合预期: {out}"
    );
}

/// 验证 POST body 回显：客户端 write+end 的 chunked 体在服务端 'data'/'end'
/// 事件聚合后回显；res.statusCode 属性赋值改状态码。
#[test]
fn http_post_echo_matches_go() {
    let work = work_dir("post_echo");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const q = (v) => JSON.stringify(v) === undefined ? '\"' + v + '\"' : JSON.stringify(v);\n",
            "const server = http.createServer((req, res) => {\n",
            "    let body = \"\";\n",
            "    req.on(\"data\", (c) => { body += c.toString(); });\n",
            "    req.on(\"end\", () => {\n",
            "        res.statusCode = 201;\n",
            "        res.setHeader(\"X-A\", \"1\");\n",
            "        res.setHeader(\"x-multi\", [\"a\", \"b\"]);\n",
            "        res.end(\"echo:\" + body);\n",
            "    });\n",
            "});\n",
            "server.listen(47143, \"127.0.0.1\", () => {\n",
            "    const req = http.request({ host: \"127.0.0.1\", port: 47143, path: \"/post\", method: \"POST\", headers: { \"X-Req\": \"zz\" } }, (res) => {\n",
            "        console.log(\"status:\", res.statusCode, q(res.statusMessage));\n",
            "        console.log(\"xa:\", res.headers[\"x-a\"], \"multi:\", q(res.headers[\"x-multi\"]));\n",
            "        let b = \"\";\n",
            "        res.on(\"data\", (c) => { b += c.toString(); });\n",
            "        res.on(\"end\", () => { console.log(\"body:\", b); server.close(); });\n",
            "    });\n",
            "    console.log(\"write:\", req.write(\"pay-1\"));\n",
            "    req.end(\"pay-2\");\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("status: 201 \"201 Created\"")
            && out.contains("xa: 1 multi: \"a, b\"")
            && out.contains("body: echo:pay-1pay-2"),
        "POST 回显输出不符合预期: {out}"
    );
}

/// 验证 ServerResponse.writeHead：writeHead 后 setHeader 不上线（Go 头冻结
/// 语义）、自定义头生效、204 无 Content-Length/Content-Type。
#[test]
fn http_write_head_and_204_matches_go() {
    let work = work_dir("write_head");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const q = (v) => JSON.stringify(v) === undefined ? '\"' + v + '\"' : JSON.stringify(v);\n",
            "const absent = (v) => v === undefined || v === null;\n",
            "const server = http.createServer((req, res) => {\n",
            "    if (req.url === \"/404/404\") {\n",
            "        res.writeHead(404, \"Not Found Custom\", { \"X-Custom\": \"abc\" });\n",
            "        res.setHeader(\"x-two\", \"2\");\n",
            "        res.end(\"nope\");\n",
            "        return;\n",
            "    }\n",
            "    if (req.url === \"/empty/empty\") {\n",
            "        res.writeHead(204);\n",
            "        res.end();\n",
            "        return;\n",
            "    }\n",
            "    res.end(\"ok\");\n",
            "});\n",
            "server.listen(47145, \"127.0.0.1\", () => {\n",
            "    const get = (path, cb) => http.get({ host: \"127.0.0.1\", port: 47145, path }, cb);\n",
            "    get(\"/404/404\", (res) => {\n",
            "        console.log(\"1:\", res.statusCode, q(res.statusMessage), res.headers[\"x-custom\"], res.headers[\"x-two\"]);\n",
            "        res.on(\"end\", () => get(\"/empty/empty\", (r2) => {\n",
            "            console.log(\"2:\", r2.statusCode, q(r2.statusMessage), \"cl absent:\", absent(r2.headers[\"content-length\"]));\n",
            "            r2.on(\"end\", () => server.close());\n",
            "        }));\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("1: 404 \"404 Not Found\" abc undefined")
            && out.contains("2: 204 \"204 No Content\" cl absent: true"),
        "writeHead/204 输出不符合预期: {out}"
    );
}

/// 验证服务器生命周期：listen callback 先于 'listening' 事件、'connection'
/// 事件、address()/getConnections()、close callback（顺序逐字对齐）。
#[test]
fn http_server_lifecycle_matches_go() {
    let work = work_dir("lifecycle");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const s = http.createServer((req, res) => { res.end(\"ok\"); });\n",
            "console.log(\"init:\", s.listening, s.timeout, s.keepAliveTimeout, s.maxHeadersCount === null, s.headersTimeout, s.requestTimeout, s.maxRequestsPerSocket);\n",
            "console.log(\"methods:\", typeof s.listen, typeof s.close, typeof s.address, typeof s.setTimeout, typeof s.getConnections, typeof s.closeAllConnections, typeof s.closeIdleConnections);\n",
            "console.log(\"addr before:\", s.address());\n",
            "s.on(\"listening\", () => console.log(\"evt listening\"));\n",
            "s.on(\"connection\", () => console.log(\"evt connection\"));\n",
            "const t = s.setTimeout(1234, () => console.log(\"setTimeout cb, timeout now:\", s.timeout));\n",
            "console.log(\"setTimeout returns self:\", t === s);\n",
            "s.listen(47146, \"127.0.0.1\", () => {\n",
            "    console.log(\"listen cb\");\n",
            "    const a = s.address();\n",
            "    console.log(\"addr:\", a.address, a.family, a.port);\n",
            "    s.getConnections((err, n) => console.log(\"conns:\", err === null, n === 0));\n",
            "    http.get({ host: \"127.0.0.1\", port: 47146, path: \"/\" }, (res) => {\n",
            "        res.on(\"end\", () => {\n",
            "            s.close(() => console.log(\"close cb, listening now:\", s.listening));\n",
            "        });\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "init: false 0 5000 true 60000 300000 0\n",
            "methods: function function function function function function function\n",
            "addr before: null\n",
            "setTimeout cb, timeout now: 1234\n",
            "setTimeout returns self: true\n",
            "listen cb\n",
            "addr: 127.0.0.1 IPv4 47146\n",
            "conns: true true\n",
            "evt listening\n",
            "evt connection\n",
            "close cb, listening now: false",
        ),
        "生命周期输出不符合预期: {out}"
    );
}

/// 验证 STATUS_CODES 全部 12 项、METHODS 全部 34 项（顺序敏感）与模块表面。
#[test]
fn http_status_codes_methods_surface_matches_go() {
    let work = work_dir("surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const codes = http.STATUS_CODES;\n",
            "console.log(codes[200], codes[201], codes[202], codes[301], codes[302], codes[400], codes[401], codes[403], codes[404], codes[500], codes[502], codes[503]);\n",
            "let n = 0; for (const k in codes) { n++; } console.log(\"codes count:\", n);\n",
            "console.log(\"methods len:\", http.METHODS.length, \"| first:\", http.METHODS.join(\",\"));\n",
            "console.log(\"typeofs:\", typeof http.IncomingMessage, typeof http.ServerResponse, typeof http.Agent, typeof http.globalAgent, typeof http.createServer, typeof http.request, typeof http.get);\n",
            "console.log(\"validate:\", http.validateHeaderName(\"x\"), http.validateHeaderValue(\"y\"), typeof http.validateHeaderName, typeof http.validateHeaderValue);\n",
            "const im = new http.IncomingMessage();\n",
            "const q = (v) => JSON.stringify(v) === undefined ? '\"' + v + '\"' : JSON.stringify(v);\n",
            "console.log(\"im:\", im.method, q(im.url), im.httpVersion, typeof im.headers, typeof im.resume, typeof im.pause, typeof im.destroy, typeof im.unpipe);\n",
            "const sr = new http.ServerResponse();\n",
            "console.log(\"sr:\", sr.statusCode, sr.writableEnded, typeof sr.writeHead, typeof sr.write, typeof sr.end, typeof sr.setHeader, typeof sr.getHeader, typeof sr.getHeaders, typeof sr.hasHeader, typeof sr.removeHeader, typeof sr.addTrailers, typeof sr.flushHeaders, typeof sr.writeContinue, typeof sr.cork, typeof sr.uncork, typeof sr.setTimeout);\n",
            "const a = new http.Agent();\n",
            "console.log(\"agent:\", a.keepAlive, a.keepAliveMsecs, a.maxSockets, a.maxFreeSockets, a.getName(), typeof a.createConnection, typeof a.destroy, typeof a.sockets, typeof a.freeSockets, typeof a.requests);\n",
            "const a2 = new http.Agent({ keepAlive: true, keepAliveMsecs: 3000, maxSockets: 7 });\n",
            "console.log(\"agent2:\", a2.keepAlive, a2.keepAliveMsecs, a2.maxSockets);\n",
            "console.log(\"global:\", http.globalAgent.keepAlive, http.globalAgent.keepAliveMsecs, http.globalAgent.maxFreeSockets, typeof http.globalAgent.destroy, typeof http.globalAgent.sockets, http.globalAgent.maxSockets);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("OK Created Accepted Moved Permanently Found Bad Request Unauthorized Forbidden Not Found Internal Server Error Bad Gateway Service Unavailable")
            && out.contains("codes count: 12")
            && out.contains("methods len: 34 | first: ACL,BIND,CHECKOUT,CONNECT,COPY,DELETE,GET,HEAD,LINK,LOCK,M-SEARCH,MERGE,MKACTIVITY,MKCALENDAR,MKCOL,MOVE,NOTIFY,OPTIONS,PATCH,POST,PROPFIND,PROPPATCH,PURGE,PUT,REBIND,REPORT,SEARCH,SOURCE,SUBSCRIBE,TRACE,UNBIND,UNLINK,UNLOCK,UNSUBSCRIBE")
            && out.contains("im:") && out.contains("1.1 object function function function function")
            && out.contains("agent: false 1000 Infinity 256 http")
            && out.contains("global: true 1000 256 function object undefined"),
        "表面输出不符合预期: {out}"
    );
}

/// 验证无 handler 的服务器回 500 "no handler"（Go 兜底行为）与
/// 字符串 URL 形式的 http.get。
#[test]
fn http_no_handler_and_string_url_matches_go() {
    let work = work_dir("no_handler");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const server = http.createServer();\n",
            "server.listen(47156, \"127.0.0.1\", () => {\n",
            "    const req = http.get({ host: \"127.0.0.1\", port: 47156, path: \"/x\" }, (res) => {\n",
            "        console.log(\"status:\", res.statusCode, res.statusMessage, \"clen:\", res.headers[\"content-length\"]);\n",
            "        let b = \"\";\n",
            "        res.on(\"data\", (c) => (b += c.toString()));\n",
            "        res.on(\"end\", () => { console.log(\"body:\", b); server.close(); });\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("status: 500 500 Internal Server Error clen: 10")
            && out.contains("body: no handler"),
        "no handler 输出不符合预期: {out}"
    );

    let work2 = work_dir("string_url");
    std::fs::write(
        work2.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const server = http.createServer((req, res) => { res.end(\"url:\" + req.url); });\n",
            "server.listen(47155, \"127.0.0.1\", () => {\n",
            "    http.get(\"http://127.0.0.1:47155/hello?x=1\", (res) => {\n",
            "        let b = \"\";\n",
            "        res.on(\"data\", (c) => (b += c.toString()));\n",
            "        res.on(\"end\", () => { console.log(b); server.close(); });\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out2 = common::assert_e2e_matches_go(&work2, "probe.js");
    assert_eq!(out2, "url:/hello?x=1", "字符串 URL 输出不符合预期: {out2}");
}

/// 验证顺序多次请求（keep-alive 语义下逐请求应答）与 res 'finish'/'close'
/// 事件、once/off 监听器语义。
#[test]
fn http_sequential_and_res_events_matches_go() {
    let work = work_dir("sequential");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"node:http\");\n",
            "const server = http.createServer((req, res) => {\n",
            "    res.once(\"finish\", () => console.log(\"evt finish\"));\n",
            "    const off = () => console.log(\"should not fire\");\n",
            "    res.on(\"close\", off);\n",
            "    res.off(\"close\", off);\n",
            "    res.write(\"a\");\n",
            "    res.write(\"b\");\n",
            "    res.end(\"c\");\n",
            "});\n",
            "server.listen(47157, \"127.0.0.1\", () => {\n",
            "    let i = 0;\n",
            "    const next = () => {\n",
            "        if (i === 3) { server.close(); return; }\n",
            "        http.get({ host: \"127.0.0.1\", port: 47157, path: \"/\" + i }, (res) => {\n",
            "            let b = \"\";\n",
            "            res.on(\"data\", (c) => (b += c.toString()));\n",
            "            res.on(\"end\", () => { console.log(b); i++; next(); });\n",
            "        });\n",
            "    };\n",
            "    next();\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out, "evt finish\nabc\nevt finish\nabc\nevt finish\nabc",
        "顺序请求与 res 事件输出不符合预期: {out}"
    );
}

/// 验证 https 选项校验错误消息逐字对齐（缺失/空串/无效 PEM 三类）与模块表面。
#[test]
fn https_validation_and_surface_matches_go() {
    let work = work_dir("https_validation");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const https = require(\"node:https\");\n",
            "console.log(\"typeofs:\", typeof https.createServer, typeof https.request, typeof https.get, typeof https.Agent, typeof https.globalAgent, typeof https.Server);\n",
            "try { https.createServer({}); } catch (e) { console.log(\"E1:\", e.message); }\n",
            "try { https.createServer({ key: \"k\" }); } catch (e) { console.log(\"E2:\", e.message); }\n",
            "try { https.createServer({ key: \"\", cert: \"\" }); } catch (e) { console.log(\"E3:\", e.message); }\n",
            "console.log(\"ga:\", https.globalAgent.keepAlive, https.globalAgent.maxFreeSockets);\n",
            "const s = new https.Server();\n",
            "let n = 0; for (const k in s) { n++; } console.log(\"server instance:\", typeof s, n);\n",
            "const a = new https.Agent({ keepAlive: true });\n",
            "console.log(\"agent:\", a.keepAlive, a.maxFreeSockets, a.getName());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains(
            "E1: https: invalid key/cert: tls: failed to find any PEM data in certificate input"
        ) && out.contains(
            "E2: https: invalid key/cert: tls: failed to find any PEM data in certificate input"
        ) && out.contains("E3: https: createServer requires { key, cert } PEM options")
            && out.contains("server instance: object 0"),
        "https 校验输出不符合预期: {out}"
    );
}

/// 验证 http2 表面：constants 全量、getDefaultSettings、getPackedSettings、
/// sensitiveHeaders、createSecureServer 校验错误、connect 会话表面与
/// 'connect'/'close' 事件时序。
#[test]
fn http2_surface_and_events_matches_go() {
    let work = work_dir("http2_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http2 = require(\"node:http2\");\n",
            "const c = http2.constants;\n",
            "console.log(\"pseudo:\", c.HTTP2_HEADER_METHOD, c.HTTP2_HEADER_PATH, c.HTTP2_HEADER_SCHEME, c.HTTP2_HEADER_AUTHORITY, c.HTTP2_HEADER_STATUS, c.HTTP2_HEADER_PROTOCOL);\n",
            "console.log(\"err:\", c.NGHTTP2_NO_ERROR, c.NGHTTP2_PROTOCOL_ERROR, c.NGHTTP2_INTERNAL_ERROR, c.NGHTTP2_FLOW_CONTROL_ERROR, c.NGHTTP2_SETTINGS_TIMEOUT, c.NGHTTP2_STREAM_CLOSED, c.NGHTTP2_FRAME_SIZE_ERROR, c.NGHTTP2_REFUSED_STREAM, c.NGHTTP2_CANCEL, c.NGHTTP2_COMPRESSION_ERROR, c.NGHTTP2_CONNECT_ERROR, c.NGHTTP2_ENHANCE_YOUR_CALM, c.NGHTTP2_INADEQUATE_SECURITY, c.NGHTTP2_HTTP_1_1_REQUIRED, c.NGHTTP2_ERR_NOMEM);\n",
            "console.log(\"frames:\", c.HTTP2_FRAME_HEADERS, c.HTTP2_FRAME_SETTINGS, c.HTTP2_FRAME_PING, c.HTTP2_FRAME_GOAWAY);\n",
            "console.log(\"settings:\", c.HTTP2_SETTINGS_HEADER_TABLE_SIZE, c.HTTP2_SETTINGS_ENABLE_PUSH, c.HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS, c.HTTP2_SETTINGS_INITIAL_WINDOW_SIZE, c.HTTP2_SETTINGS_MAX_FRAME_SIZE, c.HTTP2_SETTINGS_MAX_HEADER_LIST_SIZE, c.HTTP2_SETTINGS_ENABLE_CONNECT_PROTOCOL);\n",
            "const d = http2.getDefaultSettings();\n",
            "console.log(\"default:\", d.headerTableSize, d.enablePush, d.initialWindowSize, d.maxFrameSize, d.maxConcurrentStreams, d.maxHeaderSize, d.maxHeaderListSize, d.enableConnectProtocol);\n",
            "console.log(\"packed:\", http2.getPackedSettings().length, typeof http2.getUnpackedSettings);\n",
            "console.log(\"sensitive:\", http2.sensitiveHeaders);\n",
            "console.log(\"typeofs:\", typeof http2.connect, typeof http2.createServer, typeof http2.createSecureServer);\n",
            "try { http2.createSecureServer({}); } catch (e) { console.log(\"secure err:\", e.message); }\n",
            "const sess = http2.connect(\"https://127.0.0.1:47150\", () => {});\n",
            "console.log(\"sess:\", typeof sess.request, typeof sess.close, typeof sess.ref, typeof sess.unref);\n",
            "sess.on(\"connect\", (s) => { console.log(\"connect evt:\", s === sess); sess.close(() => { console.log(\"close cb\"); }); });\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("default: 4096 true 65535 16384 4294967295 65535 65535 false")
            && out.contains("packed: 0 function")
            && out.contains("sensitive: Symbol(sensitiveHeaders)")
            && out.contains("secure err: tls: createServer requires { key, cert } PEM options")
            && out.contains("sess: function function function function")
            && out.contains("connect evt: true")
            && out.contains("close cb"),
        "http2 表面输出不符合预期: {out}"
    );
}

/// 验证 http2.createServer（Go 即复用 node:http 明文 Server）：可监听、
/// 可用 http 客户端完成往返后 close 退出。
#[test]
fn http2_create_server_plain_roundtrip_matches_go() {
    let work = work_dir("http2_server");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http2 = require(\"node:http2\");\n",
            "const http = require(\"node:http\");\n",
            "const srv = http2.createServer((req, res) => { res.end(\"h2-plain\"); });\n",
            "srv.listen(47159, \"127.0.0.1\", () => {\n",
            "    http.get({ host: \"127.0.0.1\", port: 47159, path: \"/z\" }, (res) => {\n",
            "        let b = \"\";\n",
            "        res.on(\"data\", (c) => (b += c.toString()));\n",
            "        res.on(\"end\", () => { console.log(b); srv.close(); });\n",
            "    });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out, "h2-plain",
        "http2.createServer 往返输出不符合预期: {out}"
    );
}
