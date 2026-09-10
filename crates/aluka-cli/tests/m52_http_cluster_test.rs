//! M5.2 修复轮端到端对拍测试（Node.js 22 LTS 为唯一权威）：
//!
//! 1. `server.listen` 失败的 `'error'` 载荷为真 `Error` 实例（`code`/`errno`/
//!    `syscall`/`address`/`port`），且**异步**派发（不在 `listen()` 调用栈内），
//!    `server.listening === false`；无监听器时按 EventEmitter 语义上抛未捕获异常。
//!    覆盖 `net` 与 `http`（`https` 复用 `http` 的 `server_listen`，同一代码路径）。
//!
//! 探针纪律：端口一律 `listen(0)` 取随机端口（避免固定端口并发干扰），
//! 全部 127.0.0.1 回环，结尾关闭全部实体保证 aluvm 正常退出。

mod common;

use std::path::PathBuf;
use std::process::Command;

/// 创建隔离的临时测试目录。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("m52_http_cluster_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

// --- 项 1：listen 失败的 'error' 载荷与时机 --------------------------------

/// `net` 服务器端口冲突：`'error'` 载荷属性 + 异步时机 + `listening` 语义。
#[test]
fn net_listen_eaddrinuse_error_payload_matches_node() {
    let work = work_dir("net_listen_err");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"net\");\n",
            // 顺序标记：若 'error' 异步派发，应为 before-listen,after-listen,error
            "const order = [];\n",
            "const blocker = net.createServer().listen(0, \"127.0.0.1\", () => {\n",
            "  const port = blocker.address().port;\n",
            "  const srv = net.createServer();\n",
            "  srv.on(\"error\", (e) => {\n",
            "    order.push(\"error\");\n",
            "    console.log(\"isError=\" + (e instanceof Error), \"name=\" + e.name,\n",
            "      \"code=\" + e.code, \"errno=\" + e.errno, \"syscall=\" + e.syscall,\n",
            "      \"address=\" + e.address,\n",
            "      \"portIsNumber=\" + (typeof e.port === \"number\" && e.port > 0));\n",
            "    console.log(\"message=\" + e.message.replace(/:\\d+$/, \":PORT\"));\n",
            "    console.log(\"listening=\" + srv.listening);\n",
            "    console.log(\"order=\" + order.join(\",\"));\n",
            "    blocker.close();\n",
            "  });\n",
            "  order.push(\"before-listen\");\n",
            "  const ret = srv.listen(port, \"127.0.0.1\");\n",
            "  order.push(\"after-listen\");\n",
            // listen() 返回 Server 自身（Node 语义）
            "  console.log(\"returns-self=\" + (ret === srv));\n",
            "});\n",
        ),
    )
    .expect("写探针失败");
    common::assert_e2e_matches_node(&work, "probe.js");
}

/// `http` 服务器端口冲突：与 `net` 同一 Node 语义（`https` 复用该路径）。
#[test]
fn http_listen_eaddrinuse_error_payload_matches_node() {
    let work = work_dir("http_listen_err");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const http = require(\"http\");\n",
            "const order = [];\n",
            "const blocker = http.createServer();\n",
            "blocker.listen(0, \"127.0.0.1\", () => {\n",
            "  const port = blocker.address().port;\n",
            "  const srv = http.createServer(() => {});\n",
            "  srv.on(\"error\", (e) => {\n",
            "    order.push(\"error\");\n",
            "    console.log(\"isError=\" + (e instanceof Error), \"name=\" + e.name,\n",
            "      \"code=\" + e.code, \"errno=\" + e.errno, \"syscall=\" + e.syscall,\n",
            "      \"address=\" + e.address,\n",
            "      \"portIsNumber=\" + (typeof e.port === \"number\" && e.port > 0));\n",
            "    console.log(\"message=\" + e.message.replace(/:\\d+$/, \":PORT\"));\n",
            "    console.log(\"listening=\" + srv.listening);\n",
            "    console.log(\"order=\" + order.join(\",\"));\n",
            "    blocker.close();\n",
            "  });\n",
            "  order.push(\"before-listen\");\n",
            "  srv.listen(port, \"127.0.0.1\");\n",
            "  order.push(\"after-listen\");\n",
            "});\n",
        ),
    )
    .expect("写探针失败");
    common::assert_e2e_matches_node(&work, "probe.js");
}

/// 无 `'error'` 监听器：EventEmitter 默认行为 = 上抛未捕获异常（进程退出码非 0）。
///
/// Node 与 aluka 的堆栈文本不同，故对拍「退出码非 0 + stderr 含 EADDRINUSE +
/// stdout 输出在异常前的内容一致」。
#[test]
fn listen_error_without_listener_throws_uncaught_like_node() {
    let work = work_dir("listen_uncaught");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"net\");\n",
            "const blocker = net.createServer().listen(0, \"127.0.0.1\", () => {\n",
            "  const port = blocker.address().port;\n",
            "  const srv = net.createServer();\n",
            "  srv.listen(port, \"127.0.0.1\");\n",
            // 未注册 'error'：异常在后续泵轮抛出，故本行先打印
            "  console.log(\"listen-called\");\n",
            "});\n",
        ),
    )
    .expect("写探针失败");
    common::compile_all_js(&work);
    let bc = work.join("probe.bc");
    let out = Command::new(common::aluvm_exe())
        .arg("run")
        .arg(&bc)
        .current_dir(&work)
        .output()
        .expect("运行 aluvm 失败");
    let rust_stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    let rust_stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        !out.status.success(),
        "aluka：无 'error' 监听器时必须上抛未捕获异常（非 0 退出码），实际退出码 0"
    );
    assert_eq!(rust_stdout.trim(), "listen-called", "aluka 异常前输出不符");
    assert!(
        rust_stderr.contains("EADDRINUSE"),
        "aluka 未捕获异常文案应含 EADDRINUSE，实际 stderr:\n{rust_stderr}"
    );

    if !common::node_available() {
        eprintln!("[SKIP node-e2e] listen_uncaught: 本机无 node，仅验证 aluka 自身行为");
        return;
    }
    let node = common::node_run_outcome(&work.join("probe.js"));
    assert!(node.spawned, "node 无法启动：{}", node.stderr);
    assert!(
        !node.ok,
        "Node 侧未抛出未捕获异常（对拍不成立），stdout:\n{}",
        node.stdout
    );
    assert_eq!(node.stdout.trim(), rust_stdout.trim(), "异常前输出不一致");
    assert!(
        node.stderr.contains("EADDRINUSE"),
        "Node stderr 应含 EADDRINUSE，实际:\n{}",
        node.stderr
    );
}
