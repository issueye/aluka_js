//! Phase 7 交互 I/O 内置库端到端与接口对拍测试：
//! - `sqlite`（DatabaseSync 建表/插入/查询/更新/迭代/事务/错误路径，rusqlite
//!   真实执行 SQL，与 Go modernc 驱动逐字对拍）；
//! - `readline`（Interface 表面、question 阻塞读行、'line'/'close' 事件、
//!   piped stdin 逐字对拍）；
//! - `readline/promises`（Promise 化 question：流消费 / stdin 回退）；
//! - `repl`（start 读行-求值循环、REPLServer 方法面、piped stdin 对拍）；
//! - `tty`（isatty、ReadStream/WriteStream 构造与 ERR_TTY_INIT_FAILED）。
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。

mod common;

use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase7_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 以管道 stdin 运行 aluvm（Go 编译产出的 .bc），返回 trim 后的 stdout。
fn aluvm_run_with_stdin(bc: &Path, input: &str) -> String {
    let mut child = Command::new(common::aluvm_exe())
        .arg("run")
        .arg(bc)
        .current_dir(bc.parent().expect("bc 有父目录"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("运行 aluvm 失败");
    // 小体积输入一次写入后立即关闭，避免管道死锁。
    child
        .stdin
        .as_mut()
        .expect("stdin 已管道化")
        .write_all(input.as_bytes())
        .expect("写入 stdin 失败");
    let out = child.wait_with_output().expect("等待 aluvm 失败");
    assert!(
        out.status.success(),
        "aluvm 执行失败: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 管道 stdin 版标准 e2e：原生编译 → aluvm 管道执行。
fn assert_piped_e2e_matches_go(work: &Path, entry: &str, input: &str) -> String {
    common::compile_all_js(work);
    let bc = work.join(entry.replace(".js", ".bc"));
    aluvm_run_with_stdin(&bc, input)
}

/// sqlite：建表/插入/查询/更新/删除/迭代/列信息 全链路（:memory:）。
#[test]
fn sqlite_memory_roundtrip_e2e_matches_go() {
    let work = work_dir("sqlite_roundtrip");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { DatabaseSync } = require(\"node:sqlite\");\n",
            "const util = require(\"util\");\n",
            "const db = new DatabaseSync(\":memory:\");\n",
            "db.exec(\"CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age REAL, nick TEXT)\");\n",
            "const ins = db.prepare(\"INSERT INTO users (name, age, nick) VALUES (?, ?, ?)\");\n",
            "const r1 = ins.run(\"Alice\", 30.5, null);\n",
            "console.log(\"r1:\", util.inspect(r1), r1.changes, r1.lastInsertRowid);\n",
            "const r2 = ins.run(\"Bob\", 25, \"bobby\");\n",
            "console.log(\"r2:\", r2.changes, r2.lastInsertRowid);\n",
            "const all = db.prepare(\"SELECT * FROM users ORDER BY id\").all();\n",
            "console.log(\"rows:\", all.map(r => r.id + \":\" + r.name + \":\" + r.age + \":\" + r.nick).join(\" | \"));\n",
            "const one = db.prepare(\"SELECT * FROM users WHERE id = ?\").get(2);\n",
            "console.log(\"get2:\", one.id, one.name, one.age, one.nick);\n",
            "const none = db.prepare(\"SELECT * FROM users WHERE id = 999\").get();\n",
            "console.log(\"none:\", none === undefined, none === null);\n",
            "const agg = db.prepare(\"SELECT COUNT(*) AS c, SUM(age) AS s FROM users\").get();\n",
            "console.log(\"agg:\", agg.c, agg.s);\n",
            "const upd = db.prepare(\"UPDATE users SET age = ? WHERE name = ?\").run(31, \"Alice\");\n",
            "console.log(\"upd:\", util.inspect(upd));\n",
            "const named = db.prepare(\"SELECT * FROM users WHERE name = :name\").get({ name: \"Bob\" });\n",
            "console.log(\"named:\", named.id, named.nick);\n",
            "const it = db.prepare(\"SELECT name FROM users ORDER BY id\").iterate();\n",
            "const n1 = it.next(); const n2 = it.next(); const n3 = it.next();\n",
            "console.log(\"iter:\", n1.value.name, n2.value.name, n3.done, typeof it.next);\n",
            "const cols = db.prepare(\"SELECT id, name FROM users\").columns();\n",
            "console.log(\"cols:\", cols.map(c => c.name + \"/\" + c[\"type\"]).join(\",\"));\n",
            "const del = db.prepare(\"DELETE FROM users WHERE id = 1\").run();\n",
            "console.log(\"del:\", util.inspect(del));\n",
            "db.close();\n",
            "console.log(\"closed:\", db.isOpen);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("rows: 1:Alice:30.5:null | 2:Bob:25:bobby"));
    assert!(out.contains("closed: false"));
}

/// sqlite：文件库 DROP/CREATE 幂等 reopen（测试临时目录内的相对路径）。
#[test]
fn sqlite_file_db_reopen_e2e_matches_go() {
    let work = work_dir("sqlite_file_db");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { DatabaseSync } = require(\"node:sqlite\");\n",
            "const db = new DatabaseSync(\"phase7_io.db\");\n",
            "console.log(\"open:\", db.isOpen);\n",
            "db.exec(\"DROP TABLE IF EXISTS kv\");\n",
            "db.exec(\"CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT)\");\n",
            "const ins = db.prepare(\"INSERT INTO kv (k, v) VALUES (?, ?)\");\n",
            "ins.run(\"lang\", \"aluka\");\n",
            "ins.run(\"phase\", \"7\");\n",
            "const rows = db.prepare(\"SELECT k, v FROM kv ORDER BY k\").all();\n",
            "console.log(\"rows:\", rows.map(r => r.k + \"=\" + r.v).join(\",\"));\n",
            "db.close();\n",
            "console.log(\"closed:\", db.isOpen);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("rows: lang=aluka,phase=7"));
}

/// sqlite：bigint（setReadBigInts）/Buffer BLOB/布尔参数/缺参/未知命名参数/
/// 唯一约束等错误路径。
#[test]
fn sqlite_bigint_blob_errors_e2e_matches_go() {
    let work = work_dir("sqlite_edges_ta");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { DatabaseSync } = require(\"node:sqlite\");\n",
            "const { Buffer } = require(\"node:buffer\");\n",
            "const db = new DatabaseSync(\":memory:\");\n",
            "db.exec(\"CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB)\");\n",
            "db.prepare(\"INSERT INTO blobs (data) VALUES (?)\").run(Buffer.from([1, 2, 3, 250]));\n",
            "const r = db.prepare(\"SELECT data FROM blobs\").get();\n",
            "console.log(\"blob:\", typeof r.data, Buffer.isBuffer(r.data), [r.data[0], r.data[1], r.data[2], r.data[3]]);\n",
            "db.prepare(\"INSERT INTO blobs (data) VALUES (?)\").run(10n);\n",
            "const nums = db.prepare(\"SELECT id, typeof(data) AS t FROM blobs\").all();\n",
            "console.log(\"types:\", nums.map(x => x.id + \":\" + x.t).join(\" | \"));\n",
            "const st = db.prepare(\"SELECT id FROM blobs WHERE id > ?\");\n",
            "st.setReadBigInts(true);\n",
            "console.log(\"bigint read:\", typeof st.get(0).id, st.get(0).id);\n",
            "try { db.prepare(\"SELECT ?\").get(true); } catch (e) { console.log(\"bool:\", e.name, e.message); }\n",
            "const sp = db.prepare(\"SELECT ? AS v, ? AS w\");\n",
            "try { const gv = sp.get(1); console.log(\"missing-ok:\", gv.v, gv.w); } catch (e) { console.log(\"missing-err:\", e.message); }\n",
            "try { db.prepare(\"SELECT :x AS v\").get({ y: 1 }); } catch (e) { console.log(\"unknown:\", e.message); }\n",
            "db.exec(\"CREATE TABLE u (id INTEGER PRIMARY KEY, name TEXT)\");\n",
            "db.prepare(\"INSERT INTO u (id, name) VALUES (1, 'a')\").run();\n",
            "try { db.prepare(\"INSERT INTO u (id, name) VALUES (1, 'dup')\").run(); } catch (e) { console.log(\"dup:\", e.message); }\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    // M5.3 对齐 Node 22：BLOB 读出为纯 Uint8Array（非 Buffer），字节透传
    assert!(out.contains("blob: object false [ 1, 2, 3, 250 ]"));
    assert!(out.contains("bigint read: bigint 1"));
    // Node 22.23.1 实测：布尔/undefined 绑定 TypeError（无前缀、带参数序号）
    assert!(out.contains("bool: TypeError Provided value cannot be bound to SQLite parameter 1."));
    // 缺位置参数不再报错——未绑定占位按 NULL 补齐（Node 语义）
    assert!(out.contains("missing-ok: 1 null"));
    // 未知命名键文本（Node 22：Unknown named parameter，无 node:sqlite 前缀）
    assert!(out.contains("unknown: Unknown named parameter 'y'"));
    // 约束冲突消息 = errmsg 原文（无 errstr/extcode 包装）
    assert!(out.contains("dup: UNIQUE constraint failed: u.id"));
}

/// sqlite：DatabaseSync 打开不存在目录 / exec 与 prepare 语法错误。
#[test]
fn sqlite_open_and_sql_errors_e2e_matches_go() {
    let work = work_dir("sqlite_open_err");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { DatabaseSync } = require(\"node:sqlite\");\n",
            "try { new DatabaseSync(\"Z:/no/such/dir/x.db\"); } catch (e) { console.log(\"n:\", e.name, \"m:\", e.message); }\n",
            "const db = new DatabaseSync(\":memory:\");\n",
            "try { db.exec(\"NOT REAL SQL\"); } catch (e) { console.log(\"m:\", e.message); }\n",
            "try { db.prepare(\"SELECT * FROM\"); } catch (e) { console.log(\"m2:\", e.message); }\n",
            "try { db.exec(); } catch (e) { console.log(\"e0:\", e.message); }\n",
            "try { db.prepare(); } catch (e) { console.log(\"p0:\", e.message); }\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    // Node 22.23.1 实测：CANTOPEN 报 errstr 原文（无路径、无前缀、无扩展码）
    assert!(out.contains("n: Error m: unable to open database file"));
    // SQL 错误 message = errmsg 原文（无 errstr/extcode 包装）
    assert!(out.contains("m: near \"NOT\": syntax error"));
    assert!(out.contains("m2: incomplete input"));
    // 无参/非字符串 sql → TypeError（Node validator 文本）
    assert!(out.contains("e0: The \"sql\" argument must be a string."));
    assert!(out.contains("p0: The \"sql\" argument must be a string."));
}

/// sqlite：**Node 22.23.1 全链路真对拍**（评审 M5.3 验收证据补缺）——探针
/// 资源 `probes/node22_sqlite_probe.js` 覆盖 CRUD/语句结果/参数形态/iterate/
/// columns 五键/sourceSQL/exec 事务 + isTransaction/bigint/blob/绑定拒绝
/// （bool/undefined/数组 named 展开/未知命名键/缺位 NULL 补/超位越界）/
/// 错误对象 attrs（code/errcode/errstr）/close 语义/path 与 sql validator，
/// 输出与 Node 22 LTS 逐字一致（开发期 diff 见 round5 登记）。
///
/// 对拍门禁：本机 node 必须具备 `node:sqlite` 能力；缺失时打印 `[SKIP node-e2e]`
/// 标记**可见跳过**对拍侧（`cargo test -- --nocapture` 可见原文），绝不静默假装已对拍。
#[test]
fn sqlite_node22_diff_e2e_matches_node() {
    let work = work_dir("sqlite_node22_diff");
    std::fs::write(
        work.join("probe.js"),
        include_str!("probes/node22_sqlite_probe.js"),
    )
    .unwrap();
    // node 缺失 / 能力缺失 → 可见跳过对拍；能力具备 → 退出码非 0 或输出不符一律真失败。
    let out = common::assert_e2e_matches_node_with_module(&work, "probe.js", Some("node:sqlite"));
    // 锚点抽样：核心语义行必须出现（防对拍被静默跳过时失去验证）
    assert!(out.contains("cols: [{\"column\":\"id\",\"database\":\"main\",\"name\":\"id\",\"table\":\"users\",\"type\":\"INTEGER\"}"));
    assert!(out.contains("iter-end: true null"));
    assert!(out.contains("missing: 1 null"));
    assert!(out.contains(
        "dup: ERR_SQLITE_ERROR 1555 constraint failed | UNIQUE constraint failed: users.id"
    ));
    assert!(out.contains("nopath: TypeError | The \"path\" argument must be a string, Uint8Array, or URL without null bytes."));
}

/// readline：模块表面 + Interface 方法面（fake output 收集提示符）。
#[test]
fn readline_surface_e2e_matches_go() {
    let work = work_dir("readline_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const readline = require(\"node:readline\");\n",
            "console.log(\"surface:\", typeof readline.createInterface, typeof readline.emitKeypressEvents, typeof readline.clearLine, typeof readline.clearScreenDown, typeof readline.cursorTo, typeof readline.moveCursor);\n",
            "const chunks = [];\n",
            "const fakeOut = { write: (s) => { chunks.push(s); return true; } };\n",
            "const rl = readline.createInterface({ input: {}, output: fakeOut, terminal: false });\n",
            "console.log(\"terminal:\", rl.terminal, \"line:\", \"[\" + rl.line + \"]\");\n",
            "console.log(\"methods:\", typeof rl.question, typeof rl.on, typeof rl.emit, typeof rl.setPrompt, typeof rl.getPrompt, typeof rl.prompt, typeof rl.write, typeof rl.getCursorPos, typeof rl.pause, typeof rl.resume, typeof rl.close);\n",
            "console.log(\"getPrompt:\", \"[\" + rl.getPrompt() + \"]\");\n",
            "console.log(\"pos:\", rl.getCursorPos().rows, rl.getCursorPos().cols);\n",
            "rl.setPrompt(\"set> \");\n",
            "console.log(\"getPrompt2:\", \"[\" + rl.getPrompt() + \"]\");\n",
            "rl.prompt();\n",
            "console.log(\"chunks:\", \"[\" + chunks.join(\"|\") + \"]\");\n",
            "const p = rl.pause();\n",
            "console.log(\"chain:\", p === rl, rl.resume() === rl);\n",
            "console.log(\"write:\", rl.write(\"x\") === undefined);\n",
            "rl.close();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("chunks: [set> ]"));
    assert!(out.contains("chain: true true"));
}

/// readline：EOF（测试环境 stdin 为 null）下 question 打印提示符、触发
/// 'close' 且不调用回调。
#[test]
fn readline_eof_close_e2e_matches_go() {
    let work = work_dir("readline_eof");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const readline = require(\"node:readline\");\n",
            "const chunks = [];\n",
            "const rl = readline.createInterface({ input: process.stdin, output: { write: (s) => { chunks.push(s); return true; } } });\n",
            "let answered = false;\n",
            "let closed = false;\n",
            "rl.on(\"close\", () => { closed = true; });\n",
            "rl.on(\"line\", () => { answered = true; });\n",
            "rl.question(\"Name: \", () => { answered = true; });\n",
            "console.log(\"chunks:\", \"[\" + chunks.join(\"\") + \"]\");\n",
            "console.log(\"answered:\", answered, \"closed:\", closed);\n",
            "console.log(\"line:\", \"[\" + rl.line + \"]\");\n",
            "rl.close();\n",
            "console.log(\"closed-after:\", closed);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("chunks: [Name: ]"));
    assert!(out.contains("answered: false closed: true"));
    assert!(out.contains("closed-after: true"));
}

/// readline：piped stdin 下 question 读行 → 先调回调再触发 'line' 事件。
#[test]
fn readline_piped_question_e2e_matches_go() {
    let work = work_dir("readline_piped");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const readline = require(\"node:readline\");\n",
            "const rl = readline.createInterface({ input: process.stdin, output: process.stdout });\n",
            "let lineCount = 0;\n",
            "rl.on(\"line\", () => { lineCount++; });\n",
            "rl.question(\"Your name: \", (ans) => {\n",
            "  console.log(\"answer:\", \"[\" + ans + \"]\");\n",
            "  console.log(\"rl.line:\", \"[\" + rl.line + \"]\");\n",
            "  console.log(\"lines:\", lineCount);\n",
            "});\n",
            "rl.close();\n",
        ),
    )
    .unwrap();
    let out = assert_piped_e2e_matches_go(&work, "probe.js", "hello\n");
    assert_eq!(
        out,
        "Your name: answer: [hello]\nrl.line: [hello]\nlines: 0"
    );
}

/// readline/promises：fake EventEmitter 输入流上分片消费 + 无换行残尾 end
/// 兑现 + close。
#[test]
fn readline_promises_stream_e2e_matches_go() {
    let work = work_dir("rlp_stream");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const rp = require(\"readline/promises\");\n",
            "const { EventEmitter } = require(\"events\");\n",
            "const input = new EventEmitter();\n",
            "const chunks = [];\n",
            "const output = { write: (s) => { chunks.push(s); return true; } };\n",
            "const rl = rp.createInterface({ input, output });\n",
            "async function main() {\n",
            "  const p = rl.question(\"Q1: \");\n",
            "  input.emit(\"data\", \"he\");\n",
            "  input.emit(\"data\", \"llo\\n\");\n",
            "  input.emit(\"end\");\n",
            "  const line = await p;\n",
            "  const p2 = rl.question(\"Q2: \");\n",
            "  input.emit(\"data\", \"no-newline\");\n",
            "  input.emit(\"end\");\n",
            "  const l2 = await p2;\n",
            "  console.log(\"q:\", \"[\" + chunks.join(\"|\") + \"]\");\n",
            "  console.log(\"lines:\", \"[\" + line + \"]\", \"[\" + l2 + \"]\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(out, "q: [Q1: |Q2: ]\nlines: [hello] [no-newline]");
}

/// readline/promises：stdin 回退路径（piped stdin 读一行）。
#[test]
fn readline_promises_stdin_fallback_piped_e2e_matches_go() {
    let work = work_dir("rlp_fallback");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const rp = require(\"readline/promises\");\n",
            "const rl = rp.createInterface({ output: { write: () => true } });\n",
            "async function main() {\n",
            "  const line = await rl.question(\"\");\n",
            "  console.log(\"fallback-line:\", \"[\" + line + \"]\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = assert_piped_e2e_matches_go(&work, "probe.js", "pipeworld\n");
    assert_eq!(out, "fallback-line: [pipeworld]");
}

/// repl：piped stdin 下读行-求值循环（自定义 eval + cb 输出）、空行跳过、
/// `.exit` 退出、REPLServer 方法面与 context。
#[test]
fn repl_piped_loop_e2e_matches_go() {
    let work = work_dir("repl_loop");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const repl = require(\"node:repl\");\n",
            "const r = repl.start({ prompt: \"? \", eval: (cmd, ctx, file, cb) => { cb(null, \"got[\" + cmd + \"]\"); } });\n",
            "console.log(\"END\");\n",
            "console.log(\"getPrompt:\", \"[\" + r.getPrompt() + \"]\");\n",
            "console.log(\"methods:\", typeof r.setPrompt, typeof r.displayPrompt, typeof r.defineCommand, typeof r.clearBufferedCommand, typeof r.setupHistory, typeof r.close);\n",
            "console.log(\"context:\", typeof r.context);\n",
            "console.log(\"chain:\", r.clearBufferedCommand() === r, r.close() === r);\n",
            "console.log(\"noop:\", r.defineCommand(\"f\", {}), r.setupHistory(\"/h\", () => {}));\n",
        ),
    )
    .unwrap();
    let out = assert_piped_e2e_matches_go(&work, "probe.js", "abc\n\n   \n.exit\nnever\n");
    assert_eq!(
        out,
        "? got[abc]\n? ? ? END\ngetPrompt: [? ]\nmethods: function function function function function function\ncontext: object\nchain: true true\nnoop: undefined undefined"
    );
}

/// repl：setPrompt/displayPrompt 的直接输出（无 console.log 混排，纯直写
/// 输出流对拍）。
#[test]
fn repl_display_prompt_piped_e2e_matches_go() {
    let work = work_dir("repl_display");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const repl = require(\"node:repl\");\n",
            "const r = repl.start({ prompt: \"A> \", eval: (cmd, ctx, file, cb) => { cb(null, \"[\" + cmd + \"]\"); } });\n",
            "r.setPrompt(\"B> \");\n",
            "r.displayPrompt();\n",
        ),
    )
    .unwrap();
    let out = assert_piped_e2e_matches_go(&work, "probe.js", "x\n");
    // 循环第二轮提示符（A> ）→ EOF 退出 → setPrompt 后 displayPrompt 输出 B> 。
    assert_eq!(out, "A> [x]\nA> B>");
}

/// tty：表面、isatty（重定向 stdio 下全 false）、ReadStream/WriteStream
/// 构造与 ERR_TTY_INIT_FAILED。
#[test]
fn tty_surface_e2e_matches_go() {
    let work = work_dir("tty_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const tty = require(\"node:tty\");\n",
            "console.log(\"surface:\", typeof tty.isatty, typeof tty.ReadStream, typeof tty.WriteStream);\n",
            "console.log(\"isatty:\", tty.isatty(0), tty.isatty(1), tty.isatty(2), tty.isatty(99), tty.isatty(\"x\"));\n",
            "try { new tty.ReadStream(0); console.log(\"rs0 ok\"); } catch (e) { console.log(\"rs0 throw:\", e.name, \"|\", e.code, \"|\", e.message); }\n",
            "const rs = new tty.ReadStream(-1);\n",
            "console.log(\"rs:\", rs.isTTY, rs.fd, rs.isRaw, typeof rs.setRawMode, rs.setRawMode() === undefined);\n",
            "const ws = new tty.WriteStream(-1);\n",
            "console.log(\"ws:\", ws.isTTY, ws.fd, ws.columns, ws.rows, typeof ws.clearLine, typeof ws.clearScreenDown, typeof ws.cursorTo, typeof ws.getColorDepth, typeof ws.getWindowSize, typeof ws.hasColors, typeof ws.moveCursor, typeof ws._refreshSize);\n",
            "try { new tty.WriteStream(1); } catch (e) { console.log(\"ws1 throw:\", e.name, e.code); }\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("isatty: false false false false false"));
    assert!(out.contains(
        "rs0 throw: Error | ERR_TTY_INIT_FAILED | ERR_TTY_INIT_FAILED: TTY initialization failed: uv_tty_init returned EBADF"
    ));
    assert!(out.contains("rs: false -1 false function true"));
    assert!(out.contains(
        "ws: false -1 80 24 function function function function function function function function"
    ));
}
