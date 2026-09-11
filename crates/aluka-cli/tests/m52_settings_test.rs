//! M5.2 `cluster.settings` 契约端到端对拍测试（Node.js 22 LTS 为唯一权威）。
//!
//! 覆盖 `setupPrimary` / `setupMaster` 的 settings 语义与 `fork()` 的取值面：
//!
//! 1. **默认值填充**：`setupPrimary()` 写入 `args`（= 主进程额外 CLI 参数）、
//!    `exec`（= 当前主脚本绝对路径）、`execArgv`、`silent: false`；未知键保留；
//!    旧 settings 覆盖默认值、options 覆盖前两者（浅合并）；
//! 2. **settings 对象每次重建**：`setupPrimary` 后旧引用不跟随（Node 语义，
//!    非原地改写）；
//! 3. **`settings.exec` / `settings.args` 真正生效**：worker 运行指定脚本并携带实参；
//! 4. **裸 `fork()` 回退**：未调用过 `setupPrimary` 时隐式初始化，worker 重跑当前
//!    脚本且不带额外实参；
//! 5. **`settings.cwd`** → 子进程工作目录；**`settings.silent`** → 子进程 stdio
//!    是否继承；
//! 6. **validator 文本**：`exec` 非字符串 / `args` 非数组时按 Node
//!    `child_process.fork` 文案抛 `TypeError`(`code=ERR_INVALID_ARG_TYPE`)，且为
//!    `fork()` 调用栈内**同步抛出**（非 `'error'` 事件）。
//!
//! **形态归一（两处，均为已登记偏离，非语义差异）**：
//! - **产物扩展名**：字节码流水线（`alukac` → `aluvm`）下 `__filename` 指向 `.bc`
//!   产物，Node 指向 `.js` 源码。探针以 `path.extname(__filename)` 跟随当前形态
//!   选取 spawn 目标（`target.bc` / `target.js`），打印时再把 `.bc` 归一为 `.js`。
//! - **argv 槽位**：Node `process.argv` 含 exe 段（`[exe, script, ...args]`），本运行时
//!   不含（`[script, ...args]`）。探针按「与 `__filename` 同名的段」定位脚本槽位，
//!   其后元素即实参，两侧打印同一语义面。

mod common;

use std::path::{Path, PathBuf};

/// 创建隔离的临时测试目录。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "m52_cluster_settings_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 写探针源到工作目录（`.js`；bc 流水线由 harness 统一编译出 `.bc` 兄弟产物）。
fn write(work: &Path, name: &str, src: &str) {
    std::fs::write(work.join(name), src).expect("写探针失败");
}

// --- 1/2：settings 默认值、浅合并与对象重建 ---------------------------------

/// `setupPrimary()` 默认值 + 未知键 + 浅合并 + 对象重建 + `setupMaster` 别名。
#[test]
fn cluster_settings_contract_matches_node() {
    let work = work_dir("settings_contract");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
// 产物扩展名归一：bc 流水线的默认 exec 指向 .bc 产物，打印时映射回源码名。
const base = (p) => (typeof p === 'string' ? path.basename(p).replace(/\.bc$/, '.js') : typeof p);

console.log('t1-initial:', JSON.stringify(cluster.settings));

cluster.setupPrimary();
console.log('t2-keys:', Object.keys(cluster.settings).sort().join(','));
console.log('t2-exec-base:', base(cluster.settings.exec));
console.log('t2-args:', JSON.stringify(cluster.settings.args));
console.log('t2-execArgv:', JSON.stringify(cluster.settings.execArgv));
console.log('t2-silent:', cluster.settings.silent);

cluster.setupPrimary({ bogus: 7 });
console.log('t3-bogus:', cluster.settings.bogus, 'exec-kept:', base(cluster.settings.exec));

const stale = cluster.settings;
cluster.setupPrimary({ args: ['a'] });
console.log('t4-replaced:', stale === cluster.settings, 'stale-bogus:', stale.bogus);
console.log('t4-args:', JSON.stringify(cluster.settings.args), 'exec-kept:', base(cluster.settings.exec));

cluster.setupPrimary({ silent: true });
console.log('t5-silent:', cluster.settings.silent, 'exec-kept:', base(cluster.settings.exec));
cluster.setupPrimary({ silent: false });

cluster.setupMaster({ nope: 1 });
console.log('t6-master-alias:', cluster.settings.nope, base(cluster.settings.exec));
process.exit(0);
"#,
    );
    common::assert_e2e_matches_node(&work, "probe.js");
}

// --- 3：settings.exec / settings.args 生效 ---------------------------------

/// `setupPrimary({exec, args})`：worker 运行指定脚本并携带指定实参。
#[test]
fn cluster_settings_exec_args_apply_matches_node() {
    let work = work_dir("exec_args");
    write(
        &work,
        "target.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
// argv 槽位归一：定位「与 __filename 同名」的脚本段，其后即实参。
const stem = path.basename(__filename).replace(/\.bc$/, '');
const slot = process.argv.findIndex((a) => path.basename(a).replace(/\.bc$/, '') === stem);

console.log('target-isWorker:', cluster.isWorker);
console.log('target-script-stem:', path.basename(__filename).replace(/\.bc$/, '.js'));
console.log('target-args:', JSON.stringify(process.argv.slice(slot + 1)));
process.exit(0);
"#,
    );
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
// 跟随当前运行形态选目标产物：bc 流水线 → .bc，源码模式 → .js。
const EXT = path.extname(__filename);

if (cluster.isPrimary) {
  cluster.setupPrimary({
    exec: path.resolve(__dirname, 'target' + EXT),
    args: ['X', 'Y'],
  });
  cluster.fork().on('exit', (code) => {
    console.log('primary-exit:', code);
    process.exit(0);
  });
} else {
  // 若 settings.exec 未生效（回退重跑本脚本），worker 会落到这一支。
  console.log('exec-not-applied:worker-reran-primary');
  process.exit(0);
}
"#,
    );
    common::assert_e2e_matches_node(&work, "probe.js");
}

// --- 4：裸 fork 的默认回退 -------------------------------------------------

/// 未调用 `setupPrimary` 的裸 `fork()`：隐式初始化 settings，worker 重跑当前脚本、
/// 无额外实参（与旧行为一致，不回归）。
#[test]
fn cluster_default_fork_fallback_matches_node() {
    let work = work_dir("default_fork");
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');

if (cluster.isPrimary) {
  cluster.fork().on('exit', (code) => {
    console.log('primary-exit:', code);
    process.exit(0);
  });
} else {
  const stem = path.basename(__filename).replace(/\.bc$/, '');
  const slot = process.argv.findIndex((a) => path.basename(a).replace(/\.bc$/, '') === stem);
  console.log('isWorker:', cluster.isWorker);
  console.log('worker-script-stem:', path.basename(__filename).replace(/\.bc$/, '.js'));
  console.log('worker-args:', JSON.stringify(process.argv.slice(slot + 1)));
  process.exit(0);
}
"#,
    );
    common::assert_e2e_matches_node(&work, "probe.js");
}

// --- 5：settings.cwd（子进程工作目录）--------------------------------------

/// `settings.cwd` 传递给子进程：worker 的 `process.cwd()` 为指定目录，且相对 exec
/// 按该目录解析（Node `createWorkerProcess` 传 `cwd`）。
#[test]
fn cluster_settings_cwd_matches_node() {
    let work = work_dir("cwd");
    std::fs::create_dir_all(work.join("subdir")).expect("创建 subdir 失败");
    write(
        &work,
        "cwd_target.js",
        r#"
const path = require('node:path');
console.log('worker-cwd-base:', path.basename(process.cwd()));
process.exit(0);
"#,
    );
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
const EXT = path.extname(__filename);

cluster.setupPrimary({
  exec: path.resolve(__dirname, 'cwd_target' + EXT),
  cwd: path.resolve(__dirname, 'subdir'),
  args: [],
});
// 父进程在 fork 前不写 stdout：规避父/子 stdout 缓冲差异导致的行序抖动。
cluster.fork().on('exit', (code) => {
  console.log('settings-cwd-base:', path.basename(cluster.settings.cwd));
  console.log('primary-exit:', code);
  process.exit(0);
});
"#,
    );
    common::assert_e2e_matches_node(&work, "probe.js");
}

// --- 6：settings.silent（子进程 stdio 继承与否）----------------------------

/// `settings.silent = true`：子进程 stdout 不继承（不落入父进程 stdout）。
///
/// 单场景单进程：避免父/子 stdout 缓冲交织带来的行序抖动（缓冲 artifact，
/// 非语义差异）。
#[test]
fn cluster_settings_silent_on_matches_node() {
    let work = work_dir("silent_on");
    write(
        &work,
        "silent_target.js",
        r#"
console.log('child-marked:', process.argv[process.argv.length - 1]);
process.exit(0);
"#,
    );
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
const EXT = path.extname(__filename);

cluster.setupPrimary({
  exec: path.resolve(__dirname, 'silent_target' + EXT),
  args: ['SILENT-ON'],
  silent: true,
});
cluster.fork().on('exit', (code) => {
  console.log('primary-exit:', code);
  process.exit(0);
});
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");
    assert!(
        !out.contains("child-marked:"),
        "silent:true 时子进程 stdout 必须不被继承，实际捕获到子进程输出:\n{out}"
    );
}

/// `settings.silent = false`（缺省）：子进程 stdout 继承，输出直接可见。
#[test]
fn cluster_settings_silent_off_matches_node() {
    let work = work_dir("silent_off");
    write(
        &work,
        "silent_target.js",
        r#"
console.log('child-marked:', process.argv[process.argv.length - 1]);
process.exit(0);
"#,
    );
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
const EXT = path.extname(__filename);

cluster.setupPrimary({
  exec: path.resolve(__dirname, 'silent_target' + EXT),
  args: ['SILENT-OFF'],
  silent: false,
});
// 父进程在 fork 前不写 stdout：保证「子进程行 → 父进程 exit 行」的确定性顺序。
cluster.fork().on('exit', (code) => {
  console.log('primary-exit:', code);
  process.exit(0);
});
"#,
    );
    let out = common::assert_e2e_matches_node(&work, "probe.js");
    assert!(
        out.contains("child-marked: SILENT-OFF"),
        "silent:false 时子进程 stdout 应继承并可见，实际输出:\n{out}"
    );
}

// --- 7：fork 参数校验（Node validator 文本面）------------------------------

/// `exec` 非字符串 / `args` 非数组：Node `child_process.fork` validator 文案 +
/// `code=ERR_INVALID_ARG_TYPE`，且在 `fork()` 栈内同步抛出。
#[test]
fn cluster_fork_arg_validation_matches_node() {
    let work = work_dir("validate");
    write(
        &work,
        "quiet.js",
        r#"
process.exit(0);
"#,
    );
    write(
        &work,
        "probe.js",
        r#"
const cluster = require('node:cluster');
const path = require('node:path');
const QUIET = path.resolve(__dirname, 'quiet' + path.extname(__filename));

const cases = [
  ['exec=undefined', { exec: undefined }],
  ['exec=null', { exec: null }],
  ['exec=true', { exec: true }],
  ['exec=123', { exec: 123 }],
  ['exec=NaN', { exec: NaN }],
  ['exec=[]', { exec: [] }],
  ['exec={}', { exec: {} }],
  ['exec=10n', { exec: 10n }],
  ['exec=symbol', { exec: Symbol('x') }],
  ['args=undefined', { exec: QUIET, args: undefined }],
  ['args=null', { exec: QUIET, args: null }],
  ['args=1', { exec: QUIET, args: 1 }],
  ['args="nope"', { exec: QUIET, args: 'nope' }],
  ['args=true', { exec: QUIET, args: true }],
  ['args={}', { exec: QUIET, args: {} }],
];

for (const [label, opts] of cases) {
  try {
    cluster.setupPrimary(opts);
    cluster.fork();
    console.log(label, '| ok');
  } catch (e) {
    console.log(label, '|', e.name, '|', e.code, '|', e.message);
  }
}
console.log('done');
process.exit(0);
"#,
    );
    common::assert_e2e_matches_node(&work, "probe.js");
}
