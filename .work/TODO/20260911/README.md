# 2026-09-11 · 每日 TODO（M5.2 遗留用例补交与门禁复核）

> 总 TODO 见 [../README.md](../README.md)；上一日：[20260910](../20260910/README.md)

**当前里程碑**：M5（M5.1/M5.2 收尾）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1)

工作分支：`master`（`7689191` 起点，与 `origin/master` 同步）。

---

## 1. 今日目标（可判定完成态）

1. 复核并提交上一轮（20260910 M5.2 收尾轮）**漏提**的 e2e 对拍文件
   `crates/aluka-cli/tests/m52_http_cluster_test.rs`；
2. 该文件必须通过门禁三连（fmt / clippy `-D warnings` / 全工作区测试），
   不得以「看起来没问题」结项；
3. 记录本次暴露的 Git 工具链隐患（`core.untrackedCache` 导致脏工作区谎报 clean）。

---

## 2. 待办清单（开工先登记）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | 定位并复核未提交的 M5.2 遗留用例 | `[x]` | M5.2 |
| 2 | 单文件验证 + 门禁三连 | `[x]` | 门禁 |
| 3 | 提交与证据回填 | `[x]` | 证据闭环 |
| 4 | 登记 `core.untrackedCache` 隐患与规避口径 | `[x]` | 工程流程 |

---

## 3. 达成目标证据（真实证据闭环）

### 待办 1 · 定位遗留文件

`git status` 一度报告 **working tree clean**，但：

```bash
$ git ls-files --others --exclude-standard
crates/aluka-cli/tests/m52_http_cluster_test.rs
```

该文件创建于 `2026-09-10 23:51`（即 20260910 M5.2 收尾轮期间），上一轮提交
`7689191` 只带入了 `m5_semantics_test.rs` 的 8 例，**该独立文件被漏提**。

**内容核对**：3 例 `assert_e2e_matches_node` 真实差分对拍，与 20260910 README
「待办 26」登记的验收项 1（`listen` 失败错误载体 `Error` 化 + 异步派发）一一对应，
且比 `m5_semantics_test.rs` 中的同名单例覆盖更严：

| 用例 | 覆盖点 |
|---|---|
| `net_listen_eaddrinuse_error_payload_matches_node` | `net` 端口冲突：`isError`/`name`/`code`/`errno`/`syscall`/`address`/`port` 全属性 + `order=before-listen,after-listen,error`（异步派发证据）+ `listening=false` + `listen()` 返回自身 |
| `http_listen_eaddrinuse_error_payload_matches_node` | `http` 同一代码路径（`https` 复用该 `server_listen`） |
| `listen_error_without_listener_throws_uncaught_like_node` | 无 `'error'` 监听器 → EventEmitter 默认上抛：退出码非 0 + `stderr` 含 `EADDRINUSE` + 异常前 stdout 与 Node 逐字一致 |

探针纪律符合既有约定：`listen(0)` 随机端口、全 127.0.0.1 回环、结尾关闭全部实体、
工作目录走 `std::env::temp_dir()` 隔离（`work_dir` helper 与
`builtins_phase6_proc_test.rs` 同构）。

### 待办 2 · 门禁三连（真实输出）

```bash
# 1. 格式化门禁
$ cargo fmt --all --check
FMT_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ cargo clippy --all-targets --all-features -- -D warnings
CLIPPY_EXIT=0
# 注：输出中的 "`<crate>` generated 1 warning" 均为 cargo 增量编译硬链接提示
#     （"hard linking files in the incremental compilation cache failed"），
#     非代码 lint；`-D warnings` 下退出码 0 即零 lint 告警。

# 3. 全工作区全量测试门禁
$ cargo test --workspace --all-features
TEST_EXIT=0
# 汇总（85 个 test suite）: passed=611 failed=0 ignored=1
```

单文件先行的验证（真实 Node 22.23.1 参与对拍，`node --version` = `v22.23.1`）：

```bash
$ cargo test -p aluka-cli --test m52_http_cluster_test --all-features
running 3 tests
test listen_error_without_listener_throws_uncaught_like_node ... ok
test net_listen_eaddrinuse_error_payload_matches_node ... ok
test http_listen_eaddrinuse_error_payload_matches_node ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 待办 3 · 提交证据

**提交证据**：`c8d3c3e`
`test(m5.2): 补交 listen 失败错误载体端到端对拍（net/http 异步派发 + 无监听器上抛）`
（1 file changed, 155 insertions(+)，库内登记为 LF，与仓库既有文件一致：
无 `.gitattributes`、`core.autocrlf=true`，`git cat-file` 比对同目录
`m5_semantics_test.rs` 与新文件均为 0 个 CRLF 行）。

### 待办 4 · Git 工具链隐患登记

- **现象**：`git status --short` 与 `git status`（长格式）在文件确实未跟踪时仍报
  `nothing to commit, working tree clean`；同一时刻
  `git ls-files --others --exclude-standard` 与 `git status --untracked-files=all`
  能正确列出该文件。
- **根因**：本机仓库配置 `core.untrackedCache=true`（`.git/config`），未跟踪目录缓存
  未随文件创建失效（文件由外部进程在缓存刷新窗口内写入时命中该缺陷）。
- **规避口径（本轮采用）**：提交前**以 `git ls-files --others --exclude-standard`
  作为遗漏核对手段**，不单独信任 `git status` 的 clean 结论。
- **可选彻底修复（待决策，未擅自改配置）**：`git config core.untrackedCache false`
  （仓库级），或改用 `git status --untracked-files=all` 复核。注意
  `git check-ignore -v <file>` 返回非 0（未被忽略），故与 `.gitignore` /
  `~/.gitignore_global` 无关。

---

## 4. 自动化门禁结果（全绿才可交付）

```bash
$ cargo fmt --all --check                    # FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings   # CLIPPY_EXIT=0（零 lint 告警）
$ cargo test --workspace --all-features      # TEST_EXIT=0；611 passed / 0 failed / 1 ignored（85 suites）
```

---

## 5. 复审结论与偏差记录

- **`git diff` 复审**：本次唯一变更为新增 1 个测试文件（155 行），无源码改动、
  无无关夹带；内容经逐行复核，与 20260910「待办 26」验收项 1 严格对应。
- **偏差与卡点**：本文件为**补交**，未新增运行语义；其覆盖的 `listen` 错误载体
  行为已在 `7689191` 落地。M5.2 仍未闭环项（`cluster.settings.exec/args` 生效、
  `Connection: close` 服务端语义、真 RR 调度）与 M5.4 的 LCOV 覆盖率维持
  20260910 README「待办 26」的登记状态，本周转轮未推进。
- **未验证项（诚实登记）**：门禁 `1 ignored` 为既有遗留（非本轮引入），未在本次排查。

---

## 6. 本轮追加 · M5 文档同步（表述收口）

> 触发指令：「先完成文档同步 然后逐项进行」。承接本文件前序 M5 完成情况复核。

### 6.1 开工前登记（目标 + 验收标准）

| # | 文件 / 位置 | 现状（不实表述） | 期望 | 验收 |
|---|---|---|---|---|
| 1 | `.work/TODO/README.md` 里程碑总览 M5 行 | 「M5.4 切片一已落地……余 Timer Mock 与 LCOV」 | 与 §M5 细分清单（line 296）一致：切片一 + 切片二（Timer Mock）均已落地，余 LCOV 与真 `stream.Transform` 报告器 | 两处表述一致 |
| 2 | `docs/builtins-manifest.md` 第 38/39 行 | `worker_threads`/`cluster` 标「**[已完整实现]**」，并以 `builtins_phase6_proc_test.rs` 作「逐字对拍测试证明」——该文件全部走 `assert_e2e_matches_go`（= `rust_pipeline_run` 别名，**不比 Node**） | 下调为「[已实现核心]」并附剩余缺口；证明列改指向真实 Node 对拍通道（`conformance_node22_test.rs` 的 M5 用例 / `m5_semantics_test.rs` / `m52_http_cluster_test.rs`），并显式标注旧文件为「本地锚点，未与 Node 对拍」 | 表述可被 `grep` 复核 |
| 3 | 同上 第 50 行 `sqlite` | 「`builtins_phase7_io_test.rs`（12 用例逐字对拍）」 | 实为 5 例（4 例本地锚点 + 1 例 `node:sqlite` 能力门控真对拍，缺失能力打印 `[SKIP node-e2e]`） | 用例计数与文件一致 |
| 4 | 同上 第 54 行 `test` | 「Timer Mock 未实现——`t.mock.timers` 零入口」 | Timer Mock 已于 M5.4 切片二落地（`enable`/`tick`/`setTime`/`runAll`/`reset`），补 Node 对拍出处 | 与 `m5_semantics_test.rs` 一致 |
| 5 | 同上 第 447 行汇总 | 「全部 60 项矩阵已收敛为『已完整实现』」 | 标注为 2026-09-04 快照，并列出其后下调为「已实现核心」的 5 项 | 行数与矩阵表计数一致（55 + 5 = 60） |
| 6 | `docs/builtins-plan.md` 第 51-54 行 | 「遗留 Timer Mock（`t.mock.timers`）与 LCOV 覆盖率输出未闭环」 | Timer Mock 已落地；遗留收窄为 LCOV + 真 `stream.Transform` 报告器 | 表述与事实一致 |

**红线**：只做**表述与事实对齐**，不改写历史日志（带日期的小节保留原貌，仅加「快照」限定）；
不借机做全仓链接路径批量替换（该仓 `docs/builtins-manifest.md` 的 `file:///e:/codes/go_projects/...`
链接基址整体过期，属独立问题，本轮仅登记不处理）。

### 6.2 交付摘要

**改动 3 文件（+10 / −7），无代码改动：**

| 文件 | 改动 |
|---|---|
| `.work/TODO/README.md` | 里程碑总览 M5 行表述与 §M5 细分清单对齐 |
| `docs/builtins-manifest.md` | 第 38/39 行 `worker_threads`/`cluster` 重新分级 + 真实对拍通道；第 50 行 sqlite 用例计数；第 54 行 Timer Mock 状态；第 447 行加「2026-09-04 快照」限定并列出 5 项下调 |
| `docs/builtins-plan.md` | Tier 3.5 备注的 Timer Mock 状态更新 |

复审：`git diff` 逐块核对，仅表述改动，无源码、无夹带。

**独立登记（本轮不处理）**：`docs/builtins-manifest.md` 全文 60 余处代码链接基址为
`file:///e:/codes/go_projects/aluka_lang/aluka_lang/aluka_r/...`，与本仓实际路径
`e:/code/issueye/rust_projects/aluka_js/...` 不符（该盘符/路径疑似自旧 Go 仓复制而来），
属文档整体性缺陷，需单独一轮批量修正。

---

## 7. 待办 27 · M5.2 剩余项①：`cluster.settings.exec/args` 生效

> 触发指令：「先完成文档同步 然后逐项进行」。本项 = `20260910/README.md` 待办 26
> 「未完成项 2/3」中的**项 2**（该项当时因 fixer 撞轮上限未做，登记为 ❌）。

### 7.1 开工前登记（目标 + 验收标准）

| # | 项 | 现状（已核对） | Node 22 期望 | 验收 |
|---|---|---|---|---|
| 1 | `cluster.settings.exec/args` 生效 | `cluster.rs` 存而不用；fork 恒走 `vm.entry_file` + 空参数 | `setupPrimary({exec,args})` 后 worker 执行 `exec` 并带 `args`；未设置时回退现状 | 探针对拍 + e2e；`11-cluster`/`21-m5` 不回归 |
| 2 | settings 契约（默认值/合并/重建） | `setupMaster` 只按白名单原地改写；无默认值填充；`fork` 不隐式初始化 | `setupPrimary` 每次**重建** settings（默认值 ← 旧 settings ← options 浅合并）；`fork()` 首行隐式 `setupPrimary()` | 探针对拍 |
| 3 | 关联字段 `silent` / `cwd` | 硬编码 `silent: Some(false)` / `cwd: ""` | 取 `settings.silent` / `settings.cwd` | 探针对拍 |

**红线**：Node 22 唯一权威；做不到的如实登记，不放宽断言凑绿；既有 cluster/worker
用例必须全绿。

### 7.2 Oracle 取证（先取权威语义，再动代码）

**方法**：不凭文档推断——直接取 Node v22.22.2 官方 JS 实现源码（下载件存于
`.work/scratch/m52-exec/node-primary.js`、`node-child_process.js`），并用实测探针交叉验证。

关键结论（决定实现形态）：

```js
// internal/cluster/primary.js
function setupPrimary(options) {
  const settings = {
    args: process.argv.slice(2), exec: process.argv[1],
    execArgv: process.execArgv, silent: false,
    ...cluster.settings, ...options,      // 旧 settings 覆盖默认值；options 再覆盖
  };
  cluster.settings = settings;            // 每次重建对象（非原地改写）
}
cluster.fork = function(env) {
  cluster.setupPrimary();                 // 首行隐式初始化
  const workerProcess = createWorkerProcess(id, env);
  ...
};
function createWorkerProcess(id, env) {
  return fork(cluster.settings.exec, cluster.settings.args, {
    cwd: cluster.settings.cwd, silent: cluster.settings.silent, ... });
}
// lib/child_process.js
function fork(modulePath, args = [], options) {
  modulePath = getValidatedPath(modulePath, 'modulePath');  // ← 先校验 exec
  if (args == null) args = [];
  else if (typeof args === 'object' && !ArrayIsArray(args)) { options = args; args = []; }
  else validateArray(args, 'args');
  args = [...execArgv, modulePath, ...args];               // ← 子进程 argv 布局
}
```

实测补充（对拍锚点）：
- 默认 `args` = 主进程额外 CLI 参数（**不是空数组**）；`[].slice` 语义见下；
- `exec: null` → `Received null`；`true` → `Received type boolean (true)`；`[]` →
  `Received an instance of Array`；`{}` → `Received an instance of Object`；`NaN` →
  `Received type number (NaN)`；`10n` → `Received type bigint (10n)`；
  `Symbol('x')` → `Received type symbol (Symbol(x))`；`undefined` → `Received undefined`；
- `args` 为 `null`/`undefined` → 空数组（不报错）；为**非数组对象** → 被当作 fork 的
  options（args 置空）；为其它原始类型 → `ERR_INVALID_ARG_TYPE`；
- 两类校验错误均在 `cluster.fork()` **调用栈内同步抛出**（非 `'error'` 事件）。

### 7.3 交付摘要

**源码改动（`crates/aluka-vm/src/builtins/cluster.rs`，+312/−48 量级）**：

1. `cluster_setup_master` 重写为 Node 展开语义：新建 settings 对象 → 写默认值
   （`exec` = 当前主脚本**绝对路径**、`args` = 主进程额外 CLI 参数、`execArgv: []`、
   `silent: false`）→ 旧 settings 覆盖 → options 浅合并覆盖（含未知键）。
   主脚本缺失时**不写 `exec` 键**（与 Node 的 `exec: undefined` 同形，交 validator 报错）。
2. `cluster_fork` 首行隐式 `cluster_setup_master(vm, &[])`；随后按 `createWorkerProcess`
   直取 `settings.exec` / `settings.args` / `settings.silent` / `settings.cwd` 派生子进程。
3. 新增 `settings_exec` / `settings_args` 两个校验器（+`received_repr` 文本构造），
   按 Node validator 文案抛 `TypeError(code=ERR_INVALID_ARG_TYPE)`；
   `exec` 校验先于 `args`（与 Node 调用序一致）。
4. 辅助：`current_script`（绝对化，含 `run`/`test` 子命令过滤）、`cli_args`、
   `read_silent`、`read_cwd`、`heap_string`。
5. 模块文档补齐 M5.2 契约说明与**新登记的偏离**（见 §7.5）。

**探针对拍（scratch，`.work/scratch/m52-exec/`，7 个全部逐字一致）**：

| 探针 | 覆盖面 | node vs aluka |
|---|---|---|
| `probe-settings.js` | 默认值 / 未知键 / 浅合并 / 对象重建 / `setupMaster` 别名 | **IDENTICAL** |
| `probe-default-fork.js` | 裸 fork 隐式初始化 + 回退重跑当前脚本、无实参 | **IDENTICAL** |
| `probe-exec-args.js` | `settings.exec` + `settings.args` 生效（目标文件独立） | **IDENTICAL** |
| `probe-validate-fork.js` | 15 例 validator 矩阵（含 bigint/Symbol/数组/对象/NaN） | **IDENTICAL** |
| `probe-cwd.js` | `settings.cwd` → 子进程工作目录 | **IDENTICAL**（连跑 3 次稳定） |
| `probe-silent-on.js` | `silent: true` → 子进程 stdout 不继承 | **IDENTICAL** |
| `probe-silent-off.js` | `silent: false` → 继承可见 | **IDENTICAL** |

**e2e 门禁用例（新增 `crates/aluka-cli/tests/m52_settings_test.rs`，7 例）**：
上述 7 个探针全量固化，全部走 `assert_e2e_matches_node`（bc 流水线 + Node 逐字对拍）。
`--nocapture` 确认**无 `[SKIP node-e2e]`**，即真对拍而非可见跳过：

```
$ cargo test -p aluka-cli --all-features --test m52_settings_test
running 7 tests
test cluster_settings_contract_matches_node ... ok
test cluster_settings_exec_args_apply_matches_node ... ok
test cluster_default_fork_fallback_matches_node ... ok
test cluster_settings_cwd_matches_node ... ok
test cluster_settings_silent_on_matches_node ... ok
test cluster_settings_silent_off_matches_node ... ok
test cluster_fork_arg_validation_matches_node ... ok
test result: ok. 7 passed; 0 failed; 0 ignored
```

**生产路径回归（M5 差分门禁）**：

```
$ ALUKA_CONF_FILTER=m5 cargo test -p aluka-cli --all-features --test conformance_node22_test
PASS 20-m5-worker-threads.cjs / 21-m5-cluster-http.cjs / 25-m5-structured-clone.cjs
PASS 26-m5-fetch-bodyless.cjs / 27-m5-worker-timer.cjs
Result: 5/5 passed, 0 invalid
```

### 7.4 门禁三连（真实输出）

```bash
$ cargo fmt --all --check                     # FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings   # CLIPPY_EXIT=0（零 lint 告警）
$ cargo test --workspace --all-features       # suites=86 passed=618 failed=0 ignored=1
```

- 与上一轮基线（20260911 §4：85 suites / 611 passed / 0 failed / 1 ignored）对照：
  **+1 套件、+7 用例，恰好等于本轮新增的 `m52_settings_test.rs`**；`1 ignored` 为既有
  doc-test（`builtins::builtin_module`），非本轮引入。
- 门禁日志中 `grep -cE "^test result: FAILED"` = **0**。

### 7.5 本轮新登记的偏离（诚实登记，不静默）

| 项 | Node | 本运行时 | 影响 |
|---|---|---|---|
| `settings.execArgv` 不生效 | 作为 node 旗标插在脚本前（`node <execArgv...> <exec> <args...>`） | 进程形态为 `aluka run <script>`，无旗标槽位；键仍按 Node 默认写 `[]` | 显式设置 `execArgv` 无效 |
| `settings.args` 为非数组对象 | 该对象被当作 `child_process.fork` 的 options（覆盖 `cwd`/`silent`/`stdio`） | 仅按「args 置空」处理 | 携带 options 键的对象语义不同（探针可覆盖的等价面已对齐） |
| validator `Received` 描述 | 含 `Received function <name>` 等 exotic 形 | 实现 string/number/boolean/null/undefined/bigint/Symbol/Array/Object | 函数等 exotic 值文案不同 |
| `settings.serialization`/`stdio`/`uid`/`gid`/`windowsHide` | 传入 fork | 未接线 | — |
| 真 round-robin 调度 | `schedulingPolicy` 可生效 | 恒 `SCHED_NONE`（内核分发） | 沿用待办 26 登记 |

### 7.6 工程隐患登记（本机环境，非代码缺陷）

- **增量编译缓存损坏 → rustc ICE**：本轮构建时 `aluka-builtins` / `aluka-webapi` 触发
  `rustc_metadata::encode_metadata` 处 `expect_failed`（rustc 1.95.0），且此前已有
  `failed to garbage collect finalized incremental compilation session directory ...
  拒绝访问 (os error 5)` 警告。**规避口径**：本轮全部构建/门禁用
  `CARGO_INCREMENTAL=0`，ICE 不再复现。建议后续 CI/门禁固定该变量，或清理
  `target/debug/incremental` 后重试。
- **`process.argv` 缺 exe 槽位 / `__dirname` 为相对路径**：均为既有引擎级形态偏离
  （非本轮引入）。本轮探针以「与 `__filename` 同名的 argv 段」定位脚本槽位、
  以 `path.resolve(__dirname, ...)` 绝对化，使两侧打印同一语义面；探针头部已写明
  归一理由。建议后续立专项评估是否向 Node 形态对齐（影响面超出 M5）。
- **构建耗时观察**：`CARGO_INCREMENTAL=0` 下全量测试门禁墙钟约 5 分钟（增量开启时
  约 85~117s）；本轮因 ICE 权衡取正确性优先。

---

## 8. 待办 27 · M5.2 剩余项②：服务端 `Connection: close` 语义

### 8.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | 服务端按 Node 判定写 `Connection` 头 | 请求带 `close`／响应显式设 `close`／HTTP/1.0 无 keep-alive／HTTP/1.0 + keep-alive → 响应 `Connection: close`；其余 → `Connection: keep-alive` + `Keep-Alive: timeout=5` | 五情形原始报文对拍（§8.4） |
| 2 | 判定为「最后一次」时关闭 socket | 客户端观测到服务端 FIN（`server-ended`/`server-closed` = true），且不发生残留字节截断 | 同上 |
| 3 | keep-alive 真复用不回归 | 同一 socket 串行两次请求均得响应且服务端不关；`close` 情形第二次请求不再得响应 | 复用对拍 2 例（§8.4） |
| 4 | 既有 `http`/`net`/`https`/`tls`/Express 用例不回归 | 定向套件全绿 + 门禁三连全绿 | §8.5 |

**开工前事实**：`http/server.rs` 写出响应后只调 `mark_conn_idle`（解除占用标记），
**从不关闭连接**；全 `http/` 目录无任何 `Connection:` 响应头生成。

### 8.2 Oracle 取证（先取权威语义，再动代码）

下载 Node v22.22.2 官方 JS 实现（`.work/scratch/m52-conn-close/node-_http_outgoing.js`
/ `node-_http_server.js`），语义链路如下：

| 环节 | 出处 | 语义 |
|---|---|---|
| 请求侧 keep-alive | llhttp `shouldKeepAlive` | `(Connection 含 keep-alive ‖ HTTP/1.1) && !Connection 含 close`（`close` 优先） |
| 响应默认体定界 | `_http_server.js:207-209` | HTTP/1.0 请求 → `useChunkedEncodingByDefault = (TE 含 chunked)`（本情形为假）、`shouldKeepAlive = false` |
| `Connection` 头 | `_http_outgoing.js:520-546` | ① 用户已设 → 原样保留，`_last = Connection 含 close`（`RE_CONN_CLOSE`）；② 否则 `shouldSendKeepAlive = shouldKeepAlive && (已设 Content-Length ‖ useChunkedEncodingByDefault)` → 真写 `keep-alive`（并在用户未自设 `Keep-Alive` 时补 `timeout=<_keepAliveTimeout/1000>`），假写 `close` 且 `_last = true` |
| 关连接时机 | `_http_server.js:1034-1036` | `res._last` → `socket.destroySoon()`（flush 完成后销毁） |
| 头顺序 | `_storeHeader` | 用户头 → `Date` → `Connection`(+`Keep-Alive`) → `Content-Length`/`Transfer-Encoding` |

实测锚点（`node probe.js`，5 情形；`Date` 属时间戳，比较时剔除）：

```
[req-close]         status-line=HTTP/1.1 200 OK  connection=close       content-length=5        server-ended=true
[res-close]         status-line=HTTP/1.1 200 OK  connection=close       content-length=5        server-ended=true
[http10]            status-line=HTTP/1.1 200 OK  connection=close       content-length=<absent> server-ended=true
[http10-keepalive]  status-line=HTTP/1.1 200 OK  connection=close       content-length=<absent> server-ended=true
[default]           status-line=HTTP/1.1 200 OK  connection=keep-alive  keep-alive=timeout=5    content-length=5  server-ended=false
```

两处**非显然**结论（仅靠记忆不可能得出，故必须实测）：
1. HTTP/1.0 响应**不带 `Content-Length`**，响应体以关连接定界；
2. HTTP/1.0 **即使带 `Connection: keep-alive` 也仍是 `close`** ——
   因 `shouldSendKeepAlive` 里 `useChunkedEncodingByDefault === false` 且未设 CL，
   与 `res.shouldKeepAlive = true` 无关。

### 8.3 实现要点

1. **`wire.rs`**：`RequestHead` 增 `version: (u8, u8)`；新增 `parse_http_version`（起始行
   兜底 HTTP/1.1）与 `conn_token`（等价 `RE_CONN_CLOSE` 的词边界整词匹配）、
   `header_value`；头顺序由既有 `serialize_response` 的向量顺序承载，无需改动。
2. **`state.rs`**：`Conn` 增 `close_after_write` / `fin_sent`；`RespBinding` 增
   `should_keep_alive` / `use_chunked_by_default`（请求侧判定，派发时冻结）；
   `ReqDispatch` 携带两者；`BindingSnapshot` 从 7 元组改为**具名结构体**（9 字段，
   避免不可读的长元组解构）。
3. **`server.rs`**：新增 `request_conn_policy`（请求侧两判定）、`append_conn_headers`
   （`Connection` 装配 + 返回 `_last`，`finalize_response` 与无 handler 的 500 兜底共用）、
   `mark_conn_close_after_write`。`finalize_response` 按 `用户头 → date →
   connection(+keep-alive) → content-length → content-type` 装配；`!use_chunked_by_default`
   时不写 `Content-Length`。
4. **泵**：flush 段在 `out` 落空后按 `close_after_write` 发 FIN（明文
   `shutdown(Shutdown::Write)`；TLS `send_close_notify` + `write_tls` 冲刷），
   以 `fin_sent` 纳入回收条件（对齐 `destroySoon`：flush 后即回收）。
   回收条件补 `out.is_empty()` 守卫——**顺带修掉一处既有潜在缺陷**：原实现
   `eof && !res_active` 即可回收，会在 `WouldBlock` 残留未落盘时截断响应尾字节。
   TLS 分支额外要求 `!tls.wants_write()`：`out` 清空仅代表**明文**已喂给会话
   writer，密文仍可能滞留在 rustls 内部缓冲，此时回收会连同未发出的记录一起丢弃。
5. **新增 e2e**：`crates/aluka-cli/tests/m52_conn_close_test.rs`（2 例，含逐字段
   期望串断言，防「两侧同为空白输出」的假一致）。
6. **改动顺序说明（证据完整性）**：TLS 的 `wants_write` 守卫是在首轮门禁之后补入的，
   因此**门禁三连与全部探针已针对最终代码重跑**（见 §8.4/§8.5）。

### 8.4 对拍证据（aluka vs Node 22，逐字节）

```bash
$ node probe.js > node-oracle.txt ; aluvm run probe.bc > aluka-out.txt
$ diff node-oracle.txt aluka-out.txt      # 无输出 → IDENTICAL
$ # 稳定性：probe1 ×3 / probe2 ×3 全部 IDENTICAL
probe1 IDENTICAL(1)  probe1 IDENTICAL(2)  probe1 IDENTICAL(3)
probe2 IDENTICAL(1)  probe2 IDENTICAL(2)  probe2 IDENTICAL(3)
```

- 五情形原始报文（`req-close` / `res-close` / `http10` / `http10-keepalive` / `default`）
  的 `status-line`、`connection`、`keep-alive`、`content-length`、
  `transfer-encoding`、`body`、头顺序（范围内）、`server-ended`、`server-closed`
  **全部与 Node 一致**。
- 复用对拍：`keepalive-reuse` resp-count=2 / server-ended=false；
  `req-close-no-reuse` resp-count=1 / server-ended=true —— 两侧一致。
- **TLS 关闭路径冒烟**（`probe-tls.js`：https 自签回环 + 客户端带
  `Connection: close` + 5KB body）：两侧均输出
  `STATUS 200 / CONN close / LEN 5000 / BODY-OK true / CLOSED` —— 无截断。
- 定向回归：`builtins_phase5_http_test` / `builtins_phase5_net_test` /
  `m52_http_cluster_test`（3） / `m52_settings_test`（7） / `https_tls_loopback_test`（1）
  / `m3_tls_loopback_test`（2） / `express_e2e_test`（1） 全绿。
- **证据边界（显式声明）**：TLS 路径为**行为冒烟**（JS 可见输出一致），未做报文级
  逐字节对拍（TLS 记录已加密，无法直接文本比对）；报文级对拍仅覆盖明文路径。

### 8.5 门禁三连（真实输出）

```bash
$ cargo fmt --all --check                                   # FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings   # CLIPPY_EXIT=0（零 lint 告警）
$ CARGO_INCREMENTAL=0 cargo test --workspace --all-features  # TEST_EXIT=0
```

聚合统计（**最终代码**，日志 `.work/scratch/m52-conn-close/full-test-final.log`，
即 TLS `wants_write` 守卫补入后重跑）：
`suites=87 passed=620 failed=0 ignored=1`，`grep -cE "^test result: FAILED|^error"` = **0**。

- 与上一轮基线（§7.4：86 suites / 618 passed / 0 failed / 1 ignored）对照：
  **+1 套件、+2 用例**，恰好等于本轮新增的 `m52_conn_close_test.rs`；
  `1 ignored` 为既有 doc-test（`builtins::builtin_module`），非本轮引入。

### 8.6 本轮新登记的偏离（诚实登记，不静默）

| 项 | Node | 本运行时 | 影响 |
|---|---|---|---|
| **空闲 keep-alive 连接超时清扫** | `server.keepAliveTimeout = 5000` + `keepAliveTimeoutBuffer = 1000` → 实测响应后约 **6.03s** 断连 | **无清扫**，连接长期保留（实测 7s 后仍 open） | 只广播 `Keep-Alive: timeout=5` 但**不强制断连**；长连接数量可能累积 |
| `server.keepAliveTimeout` 取值 | 可配置，广播值 = 该值/1000 | 恒广播 `timeout=5` | 自定义值不生效（`http/server.rs:60` 仅作属性表面） |
| `server.maxRequestsPerSocket` 达上限 | 写 `Connection: close` | 未接线 | 未复刻 |
| 响应 `Content-Type` 嗅探 | Node 不补 | 补 `text/plain; charset=utf-8` 等（Go 缓冲 writer 行为） | 既有偏差；本轮探针从「头顺序」对比中剔除并显式注释理由 |
| `settings.…`（承 §7.5） | — | — | 见 §7.5 |

### 8.7 工程隐患与探针纪律（本轮新增）

- **⚠️ 定时器到期模型（既有设计，尚未登记为偏离）**：`timers.rs::schedule_raw` /
  `http::state::schedule_task` 把到期时间算作「**队尾 due + delay**」（累加），
  而非「now + delay」。多定时器并存时**触发顺序即与 Node 不同**。实测（`dbg-timer.js`）：

  ```
  node : t250 @265   t400 @420   t800 @821
  aluka: t250 @429   t400 @1106  t800 @2408      # 顺序同但整体延迟；跨来源计时器会乱序
  ```
  本轮首次写复用探针时用 `setTimeout(250)` 发第二请求，实测该回调被排到 800ms 定时器
  **之后**（`dbg2.js`：`DONE → CLIENT-END → CLIENT-SEND-2`），一度误判为
  「keep-alive 复用失效」。**修正**：探针改为「收到首个响应即在 `data` 回调内发第二
  请求」的事件驱动写法，判定随即与 Node 一致。→ 建议立专项评估该模型（影响面覆盖
  `timers` 全量对拍与所有多定时器场景，超出 M5 范围）。
- **`os error 5` 写入被拒**：一次全量测试编译在写
  `target/debug/deps/builtins_phase4_events_test-*.d` 时报「拒绝访问」，重试即过
  （Windows 文件锁抖动，非代码缺陷）。`CARGO_INCREMENTAL=0` 仍是本轮固定口径
  （规避 §7.6 的 rustc ICE）。
- **构建耗时**：`CARGO_INCREMENTAL=0` 下全量门禁墙钟 5m23s~5m56s。


