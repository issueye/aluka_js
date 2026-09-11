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
| 5 | 待办 30 · M5.1 收尾：`postMessageToThread` 真线程分支 + eval worker（§14） | `[x]` | M5.1 |
| 6 | 待办 31 · M5.2 `{"t":"e"}` ack 回程（§15） | `[x]` | M5.2 |

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

### 8.8 提交证据

```
$ git commit -F -   # feat(m5.2): 服务端 Connection 语义——close/keep-alive 判定、落盘后 FIN 与 HTTP/1.0 关连接定界
[master f6e6bfb] 7 files changed, 685 insertions(+), 40 deletions(-)
 create mode 100644 crates/aluka-cli/tests/m52_conn_close_test.rs
```

只暂存目标文件（`.work/scratch/m52-conn-close/`、`.workbuddy/` 未入库）。
改动文件：`wire.rs` / `state.rs` / `server.rs` / `m52_conn_close_test.rs`（新增）/
`.work/TODO/README.md` / `.work/TODO/20260911/README.md` / `docs/builtins-manifest.md`。



## 9. 待办 27 · M5.2 剩余项③：`cluster` 的 `listening` / `disconnect` 事件

### 9.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | `cluster.on('listening', (worker, info))` | 2 实参；`info` 自有键 `addressType/address/port/fd`（`fd` 恒 `undefined` 但为自有键）；`worker.state === 'listening'` | §9.4 逐字节对拍 |
| 2 | `cluster.on('disconnect', (worker))` | **1 实参**；`state === 'disconnected'`、`isConnected() === false`、`isDead() === false`、`exitedAfterDisconnect === false`，且 worker **仍在** `cluster.workers` 表中 | 同上 |
| 3 | `worker.state` 全生命周期 | `none → online → listening → disconnected → dead` 逐事件可取 | 同上 |
| 4 | `'fork'` 异步化 | `fork()` 返回后的同步阶段事件计数为 0；`fork` 事件处 `state === 'none'`、`isConnected() === true`、`workers[id]` 已写入 | 同上 |
| 5 | 事件序 | `fork → online → listening → disconnect → exit` | 同上（逐行定位断言） |
| 6 | `listening` payload 的形态覆盖 | 显式 `127.0.0.1` / `0.0.0.0` / `::1` / 未指定 host（`address === null`）四形态均一致 | §9.4 四例对拍 |
| 7 | 既有用例不回归 | 定向套件（`cluster` / `http` / Express / `child_process`）+ 门禁三连全绿 | §9.6 |

**开工前事实**（`git show f6e6bfb` 之后的基线实测，`timeout 25`）：

```
EV fork id=1 state=undefined
primary-fork-returned            ← fork 事件先于 "primary-fork-returned"（Node 相反）
EV online id=1 state=undefined
EV exit id=1 code=143 signal=null state=undefined
```
→ 4 项缺陷：① `fork` 同步发射；② 无 `state` 属性；③ 无 `listening`；④ 无 `disconnect`。

### 9.2 Oracle 取证（先取权威语义，再动代码）

**双证据**：Node v22.22.2 官方实现（`.work/scratch/m52-cluster-events/node-primary.js`
/ `node-worker.js` / `node-child.js`）+ 本机实测 oracle。

关键源码事实：

| 环节 | 出处 | 语义 |
|---|---|---|
| `'fork'` 异步 | `primary.js:196` | `process.nextTick(emitForkNT, worker)`；`cluster.workers[id] = worker` 在**返回前**同步写入 |
| `state` 初值 | `worker.js:26` | `this.state = options.state \|\| 'none'`；`exitedAfterDisconnect = undefined` |
| `isConnected()` | `worker.js:55` | `return this.process.connected`——**与 `state` 无关**（故 `state='none'` 时已为 `true`） |
| `'online'` | `primary.js:257` | `worker.state='online'` + `worker.online` + `cluster.online(worker)` |
| `'listening'` | `primary.js:332` | `info = {addressType, address, port, fd}`；`state='listening'`；`worker.emit('listening', info)` + `cluster.emit('listening', worker, info)` |
| `'disconnect'` | `primary.js:191-211` | `isDead()` 才移除（此刻未退 → 保留）；`exitedAfterDisconnect = !!exitedAfterDisconnect`（`undefined`→`false`）；`state='disconnected'`；`worker.emit('disconnect')`（**无实参**）+ `cluster.emit('disconnect', worker)`（**1 实参**） |
| `'exit'` | `primary.js:170-190` | `!isConnected()` → 移除；`state='dead'`；`worker.emit('exit', code, signal)` + `cluster.emit('exit', worker, code, signal)` |

**探针纪律（本轮新增两条，均来自实测踩坑）**：

1. **worker 侧 `'listening'` 处理器不得同步 `close()`**：Node `internal/cluster/child.js:119`
   有 `if (!indexes.has(indexesKey)) return;` 守卫——同步 `srv.close()` 会在内部上报
   监听器之前清掉 index 键，**上报帧被短路**（探针必须把「关停+退出」推迟到
   `setImmediate`）。
2. **不依赖 primary → worker 的消息投递**：本运行时 worker 侧
   `process.on('message')` 为空实现（已登记缺口），故探针让 worker 在自身 `'listening'`
   后自行关停退出，避免把「已登记缺口」混进本项验收。

**固化 oracle**：`.work/scratch/m52-cluster-events/probe-e.js`（3/3 稳定）

```
after-fork-sync fork-events-seen=0
EV fork id=1 state=none connected=true dead=false exitedAfterDisconnect=undefined workers=1 in-table=true
EV online id=1 state=online connected=true
EV listening id=1 state=listening argc=2 connected=true workers=1
  info.addressType=4 (typeof=number)
  info.address=127.0.0.1
  info.port-is-ephemeral=true
  info.fd=undefined (typeof=undefined)
  info-keys=addressType,address,port,fd
EV disconnect id=1 state=disconnected argc=1 connected=false dead=false exitedAfterDisconnect=false workers=1
EV exit id=1 code=0 signal=null state=dead dead=true workers=
```

补充形态 oracle（`probe-h.js` / `probe-h4.js` / `probe-h6.js`）：

| listen 形态 | `info.addressType` | `info.address` |
|---|---|---|
| `listen(0, '127.0.0.1')` | 4 | `"127.0.0.1"` |
| `listen(0)`（未指定 host） | 4 | `null` |
| `listen(0, '0.0.0.0')` | 4 | `"0.0.0.0"` |
| `listen(0, '::1')` | 6 | `"::1"` |

（`listen(port)` 的 `address === null` 对应 Node `Server.prototype.listen` 的
`listenInCluster(this, null, port, 4, …)` 分支。）

### 9.3 实现要点

- `cluster.rs`：`WorkerPhase` 扩为 `None/Online/Listening/Disconnected/Dead` 五态
  并新增 `as_state_str()`；`set_phase(vm, …)` 同步镜像 `worker.state`；
  `phase_connected()` 改为「通道连通性」（`None|Online|Listening` → true，
  对齐 `process.connected` 而非 `state`）；
- `cluster_fork`：置 `None` + `exitedAfterDisconnect: undefined`；`workers[id]`
  仍**同步**写入；`'fork'` 改为入 `vm.nexttick_queue` 的 `cluster.__emitForkNT`
  （待发射句柄经线程局部队列传递——nextTick 回调以 `this === undefined` 调用，
  拿不到接收者）；
- `dispatch_worker_frame` 新增 `"l"` 分支：构造 `info`（键序
  `addressType/address/port/fd`，`fd` 为 `undefined` 自有键）→ `set_phase(Listening)`
  → worker + cluster 双向 `'listening'`；
- `emit_disconnect(vm, worker_ref)`（幂等）：`exitedAfterDisconnect = false` →
  `state='disconnected'` → `worker 'disconnect'`（无实参）+ `cluster 'disconnect'`（1 实参），
  **不动 `workers` 表**；
- `worker_exit_wrapper`：先「等通道 EOF + 排空 inbox」→ `emit_disconnect` →
  出表 → `Dead` → `close_channel` → `'exit'`；
- `worker_notify_listening(vm, address, address_type, port)`：worker 侧在 listen
  绑定成功点上报 `{"t":"l",…}` 帧，并尽力置 worker 侧 `cluster.worker.state='listening'`；
  调用点两处——`net.rs::net_server_listen` 与 `http/server.rs::server_listen`
  （Node 下 `http.Server` 继承 `net.Server`，同样经 `cluster._getServer`）。

### 9.4 对拍证据（aluka vs Node 22，逐字节）

**主链探针**（`probe-e.js`）：

```
$ diff <(node probe-e.js) <(aluvm run probe-e.bc)
IDENTICAL          # 两侧退出码均为 0
```

**四形态 payload 探针**（`probe-h*.js`）：`explicit` / `wildcard` / `zero4` / `v6`
四例 `diff` 均 `IDENTICAL`。

**新增 e2e 用例** `crates/aluka-cli/tests/m52_cluster_events_test.rs`（4 例，
`assert_e2e_matches_node` 真 Node 对拍 + 逐字段期望串防「两侧同错」）：

| 用例 | 覆盖 |
|---|---|
| `cluster_lifecycle_events_match_node` | 完整事件链 + 事件序逐行定位 + 载荷逐字段 |
| `cluster_listening_payload_wildcard_matches_node` | 未指定 host → `address=null`、`addressType=4` |
| `cluster_listening_payload_zero4_matches_node` | `0.0.0.0` → `address="0.0.0.0"`、`addressType=4` |
| `cluster_listening_payload_ipv6_matches_node` | `::1` → `addressType=6` |

连跑 3 轮全绿（含并发执行下的稳定性）。

### 9.5 本轮发现的真实缺陷与修复（关键）

**症状**：`listening` 事件**偶发丢失**（约 1.5%，`0/25` 才可能复现一次）；一度
误判为「primary 侧退出转接抢先 `close_channel` 丢帧」。

**取证过程**（临时诊断，按 pid/角色标记后定位）：

```
[DBG][pid=13568 role=W] child_send_line FAIL (io) line={"t":"o"}
[DBG][pid=13568 role=W] notify-listening SKIP env=Ok("1") conn=false
[DBG][pid=6516  role=P] exit-wrapper worker=1 established=false eof=false drained=false
```

**根因**：`cluster_ipc::read_handshake` 把「`read_line` 读超时（`READ_POLL`
= 100ms）」直接判为**非法握手**并 `continue`，此时 `stream` 被 drop → 向对端
发 RST。对端（worker）随后的第一个 IPC 帧（`online`）即 `ECONNRESET`，`connected`
被翻转为 false，此后**所有**帧被拒发，primary 侧整条 IPC 面静默失效。连接已建立
而握手帧稍后才到是**正常时序**，原实现把正常时序当异常。

**修复**（三处，均在 `cluster_ipc.rs`）：

1. `read_handshake` 改为**带总期限（`HANDSHAKE_TIMEOUT` = 5s）的重试**，`line`
   跨重试累积；仅 EOF / 超期 / 非超时 IO 错误才判失败；
2. 握手阶段改用**长读超时**（5s），转入读行循环时再调回 `READ_POLL`(100ms)——
   100ms 粒度会在对端稍慢时反复触发超时；
3. 读行循环的 `is_closed` 判定**只在「本轮无数据可读」（读超时分支）时生效**，
   内核缓冲区中已到达的帧一律先读净。

**验证**：修复前 3/200 复现；修复后 **0/250** 复现。

另配套「次序钉死」：`worker_exit_wrapper` 在发 `'disconnect'`/`'exit'` 前等本
worker 通道 EOF（上限 300ms 兜底）并排空 inbox 在途帧——判据用
`cluster_ipc::listener_spawned`（**不是**「曾经握手成功」）：accept 线程可能尚未
处理完握手，据此跳过等待会把已在内核缓冲区里的帧永久搁浅。

### 9.6 门禁三连（真实输出）

```
# 1. 格式化门禁
$ CARGO_INCREMENTAL=0 cargo fmt --all --check
FMT_CHECK_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ CARGO_INCREMENTAL=0 cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 36.59s
CLIPPY_EXIT=0

# 3. 全工作区全量测试门禁
$ CARGO_INCREMENTAL=0 cargo test --workspace --all-features
（聚合：suites=88 passed=624 failed=0 ignored=1；无 FAILED/panicked）
TEST_EXIT=0
```

汇总（88 个 test suite）: passed=624 failed=0 ignored=1
（上一轮 §8 为 87 suites / 611 passed / 1 ignored；本轮新增 `m52_cluster_events_test.rs`
4 例、`builtins_phase6_proc_test.rs` 由 1 例拆为 2 例断言、其余为既有套件。）

定向套件（`m52_cluster_events_test` / `m52_settings_test` / `builtins_phase6_proc_test`）
连跑 3 轮全绿（含并发用例执行下的稳定性）。

### 9.7 本轮新登记的偏离（诚实登记，不静默）

1. **`Object.keys` 键序为字典序**（既有全仓偏离，本轮首次被对拍暴露）：
   Node 按插入序，本运行时按字典序（`interpreter.rs` 的 `keys.sort()`）。
   故 `listening` payload 只断言键**集合**（`Object.keys(info).sort()`），
   不比较插入序。该偏离涉及解释器核心路径（影响面覆盖所有对象键枚举与
   `JSON.stringify`），属**独立专项**，不在 M5.2 内修。
2. **`cluster.disconnect()` 的 `exitedAfterDisconnect` 仍为 `false`**：Node 下
   主进程发起的 `disconnect` 会把该标志置 `true`（`Worker.prototype.disconnect`）；
   本运行时走既有 `destroy`（杀进程）路径。
3. **worker 侧 `Worker.prototype.disconnect` / `isDisconnected` 不实现**：前者需
   额外 `{"t":"d"}` 帧与 primary 侧 ack 回程；后者在 Node 中仅经 `deprecate()`
   定义、并非自有属性。
4. **`listening` 帧的 `info.address` 不做 `dns.lookup` 解析**：显式 IP 与
   `'0.0.0.0'` / `'::1'` / 未指定 host 四形态与 Node 一致；`'localhost'` 等别名
   不做解析（登记偏离）。
5. **`'fork'` 异步化的连带修正**：`builtins_phase6_proc_test.rs` 原断言
   `fork evt count: 1`（同步发射时代的产物，且该用例为 `assert_e2e_matches_go`
   **未与 Node 对拍**）→ 按 Node 语义改为同步阶段 `0` + `nextTick` 段 `1`。

### 9.8 提交证据

```
$ git commit -F -   # feat(m5.2): cluster primary 侧生命周期事件——listening/disconnect、worker.state 与异步 fork
[master eb004f3] 9 files changed, 967 insertions(+), 70 deletions(-)
 create mode 100644 crates/aluka-cli/tests/m52_cluster_events_test.rs
```

只暂存目标文件（`.work/scratch/m52-cluster-events/`、`.workbuddy/` 未入库）。
改动文件：`cluster.rs` / `cluster_ipc.rs` / `net.rs` / `http/server.rs` /
`m52_cluster_events_test.rs`（新增）/ `builtins_phase6_proc_test.rs`（旧断言按
Node 语义修正）/ `.work/TODO/README.md` / `.work/TODO/20260911/README.md` /
`docs/builtins-manifest.md`。

**提交后剩余 M5.2 缺口**：真 round-robin 调度（`schedulingPolicy` 恒
`SCHED_NONE`）；worker 侧 `process.on('message')` 接收面（worker 不激活 IPC
事件源，见模块文档缺口段）。

## 10. 待办 28 · M5.2 剩余项④：worker 侧 `process.on('message')` / `process.disconnect()`

> 触发指令：「继续M5」。本项 = §9.8「提交后剩余 M5.2 缺口」中的**项 2**（worker 侧
> `process.on('message')` 接收面）。项 1（真 RR 调度）登记为架构级缺口，见 §10.6。

### 10.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | worker 侧 `process` 真实事件器 | `on`/`addListener`/`once`/`off`/`removeListener`/`removeAllListeners`/`emit`/`listenerCount`/`listeners` 按 Node 语义（`on` 返回 `process` 自身；`emit` 返回是否有监听器；`once` 触发即自删） | §10.5 用例 1/2/3 + 逐字对拍 |
| 2 | primary → worker 消息投递 | `worker.send(v)` → worker 内 `process.on('message', (message, handle))`：2 实参、`handle === undefined`、载荷 JSON 往返逐字一致 | §10.5 用例 1 |
| 3 | worker 侧通道保活（Node channel ref 语义） | worker 内 IPC 通道建立即保活（脚本跑完不退出），与 Node 实测一致；`process.disconnect()` / 通道关闭后解除保活 | §10.5 用例 1/2 |
| 4 | worker 侧 `cluster.worker` 事件面 | `cluster.worker.on('message', …)` 收到与 `process.on('message')` 同一载荷（Node `Worker` ctor 的 process→worker 桥接）、`cluster.worker.send` 回送、`listenerCount('message')` 初值 0 | §10.5 用例 3 |
| 5 | `process.disconnect()`（worker 侧） | 返回 `undefined`、`process.connected` **同步**翻 `false`、二次调用抛 `ERR_IPC_DISCONNECTED`（文本 `IPC channel is already disconnected`） | §10.5 用例 2 |
| 6 | primary 侧 `'disconnect'`/`'exit'` 序列 | `worker 'disconnect'`（0 实参）→ `cluster 'disconnect'`（1 实参）→ `'exit'`（`state='dead'`、已出 `workers` 表、`code=0`） | §10.5 用例 2 |
| 7 | 既有 M5 用例不回归 | `m52_cluster_events_test` / `m52_settings_test` / `m52_http_cluster_test` / `builtins_phase6_proc_test` + M5 差分门禁 5/5 + 门禁三连全绿 | §10.5 |

**开工前事实**：`cluster.rs` 模块文档缺口段自述「worker 侧不激活 IPC 事件源」，
`drain_ipc_inbox` 对 `key == 0`（worker 自身通道）的帧**直接丢弃**；`process.on` 的
处理器为 `require_aliases::process_noop_event`（空实现）。

### 10.2 Oracle 取证（先取权威语义，再动代码）

**方法**：Node v22.23.1 官方实现（`lib/internal/child_process.js` 的 `Control`
类 + `_forkChild`）+ 本机实测探针（4 次连跑逐字节稳定）。

关键源码事实（决定性）：

```js
// lib/internal/child_process.js（v22.22.2 原文）
refCounted() { if (++this.#refs === 1 && !this.#refExplicitlySet) this.#channel.ref(); },
unrefCounted() { if (--this.#refs === 0 && !this.#refExplicitlySet) this.#channel.unref(); }
// _forkChild：p.unref(); setupChannel(process, p, mode);
//   process.on('newListener',      n => (n==='message'||n==='disconnect') && control.refCounted());
//   process.on('removeListener',   n => (n==='message'||n==='disconnect') && control.unrefCounted());
// internal/cluster/worker.js：Worker ctor → this.process.on('message', (m,h) => this.emit('message', m, h))
// internal/cluster/child.js  ：process.once('disconnect', …) + cluster.worker = new Worker({process})
```

**两处非显然结论（必须实测，仅凭记忆不可能得出）**：
1. **fork 出的子进程 IPC 通道「默认保活」**：worker 脚本跑完仍不退出（实测 14s 后
   仍 `connected=true`），必须显式 `process.channel.unref()` 才释放（探针 E：
   子进程内 `process.channel.unref()` 后**立即**以 0 退出）。故「worker 无监听器即可
   自然退出」在 Node 上**不成立**；
2. **`require('cluster')` 会在 worker 的 `process` 上留下内部监听器**：
   `process.listenerCount('message')` 在用户注册前已为 **1**（桥接），注册后为 2。

**固化 oracle（探针 p1/p2/p3，4/4 逐字节稳定，存会话 scratch）**：

```text
[p1 消息投递] W listeners0=1 on=function / W listeners1=2 /
  W msg={"n":1,"s":"str","arr":[1,"2",null,true],"o":{"k":"v"}} argc=2 handle=undefined connected=true listeners=2 /
  W msg="plain" / W msg=42 / W msg={"fin":true} /
  P online id=1 state=online / P send1=true / P msg={…} argc=2 handle=undefined / … / P exit code=0 signal=null state=dead
[p2 process.disconnect] W disconnect-type=function connected=true /
  W ret=undefined connected-now=false / W second code=ERR_IPC_DISCONNECTED msg=IPC channel is already disconnected /
  P online state=online / P send-go=true /
  P worker-disconnect state=disconnected argc=0 connected=false dead=false exitedAfterDisconnect=false /
  P cluster-disconnect id=1 argc=1 / P exit code=0 signal=null state=dead dead=true workers=0
[p3 worker 事件面] W worker-state=online listeners=0 / W listeners-after=1 /
  W worker.on msg={"ping":1} argc=2 handle=undefined connected=true / W send-ret=true /
  W worker.on msg={"bye":true} … / P online state=online connected=true / P send-ping=true /
  P msg={"pong":1} argc=2 handle=undefined / P send-bye=true / P exit code=0 state=dead
```

**探针纪律（本轮新增，均来自实测踩坑）**：
1. **一切「primary 先发」必须以 worker 的 `ready` 上报为门**：`'online'` 事件到达
   primary 的时刻**可能早于** worker 脚本注册 `process.on('message')`，此时先发的
   消息被丢弃（探针首版靠 400ms 定时器规避，不稳定）；
2. **primary 侧全部打印收拢进 `'exit'` 处理器**：跨进程 stdout 交错无时序保证；
   收拢后 primary 的输出必然晚于 worker 全部输出（worker 退出前已同步落盘）；
3. **worker 退出前不得留有在途帧**：末次交互由 primary 的显式消息驱动、且该消息
   不回送（否则 `'exit'` 与帧投递竞态，输出会偶发少一行）。

### 10.3 实现要点

**6 个源码文件（+ 1 个新增 e2e 文件）**：

| 文件 | 改动 |
|---|---|
| `builtins/child_process/proc_common.rs` | 实例事件器抽出**按实例句柄**的原语（`emitter_add`/`emitter_remove`/`emitter_remove_all`/`emitter_snapshot`），`inst_on/inst_once/inst_off/inst_remove_all` 改为其薄包装（单一实现源，GC 根不变） |
| `builtins/require_aliases.rs` | `process` 事件面由**空实现**改为真实事件器：`on`/`addListener`/`once`/`off`/`removeListener`/`removeAllListeners`/`emit`/`listenerCount`/`listeners`；监听器以 `process` 单例句柄为键（方法经 NativeFn 名分派，`current_receiver()` 是方法函数不是实例）；别名共用同一函数对象（Node `off === removeListener`） |
| `builtins/cluster_ipc.rs` | 子进程侧：读线程退出（对端 EOF）翻转 `connected` 并投递 `Incoming::Closed`（key 0）；新增 `child_close_channel()`（`process.disconnect()` 用：`shutdown(Write)` 发 FIN + 本地标记未连通） |
| `builtins/cluster.rs` | worker 侧全套：`worker_setup_channel(vm)` 在通道建立即激活 IPC 事件源（Node 默认保活口径）；`on_cluster_required()`（首次 `require('cluster')` 挂接 process→`cluster.worker` 桥接）；`dispatch_self_frame()`（`{"t":"m"}` → `process 'message'`）与 `worker_channel_closed()`（`'disconnect'`）；`process.disconnect()`（`ERR_IPC_DISCONNECTED` + `'disconnect'` 经 nextTick 异步派发）；`cluster.worker` 的 emitter/`isConnected`/`isDead` 表面；`cluster_ipc_busy()` 分 worker/primary 两态 |
| `interpreter.rs` | `worker_setup_channel(&mut vm)`（激活事件源）+ worker 内 `process.disconnect` 属性 + `process` 事件方法清单补 `addListener`/`off` |
| `modules.rs` | `require()` 命中内置模块后调 `cluster::on_cluster_required`（worker 内首次 require cluster 的桥接时点） |
| `tests/m52_worker_msg_test.rs`（新增） | 5 例 `assert_e2e_matches_node` 真对拍 + 逐字段期望串 |

**关键实现口径（均由实测决定，非推断）**：
1. **保活**：worker 侧通道建立即激活事件源（对齐 Node「fork 子进程通道默认保活」实测）；
   通道关闭（对端 EOF 或 `process.disconnect()`）后由泵自行注销；
2. **桥接按 require 挂接**：本运行时 cluster 模块在 `Vm` 初始化阶段统一构建（无「首次
   require」时点），故桥接移到 `require('cluster')` 处按需挂接——这样
   `process.listenerCount('message')` 的可见值与 Node 一致（require 后为 1）；
3. **`'disconnect'` 异步派发**：`process.disconnect()` 内只同步翻 `connected` 并
   `nextTick` 排队派发（实测 Node 下调用栈内后续语句仍会执行，同步发射会吞掉它们）；
4. **桥接的退出规则**：`'disconnect'` 转发给 `cluster.worker` 后，若
   `exitedAfterDisconnect` 为假值则 `process.exit(0)`（`internal/cluster/child.js` 语义）。

### 10.4 对拍证据（aluka vs Node 22.23.1，逐字节）

**固化 oracle**：5 个探针（`p1`…`p5`）在 Node v22.23.1 连跑 **4 次输出逐字节一致**，
aluka 侧连跑 **3 次输出逐字节一致且与 Node 完全一致**（`IDENTICAL=True`）：

| 探针 | 覆盖面 | 结论 |
|---|---|---|
| `p1` | primary → worker 消息投递（载荷/argc/handle/connected/listenerCount 含内部桥接） | IDENTICAL |
| `p2` | worker `process.disconnect()`（返回值/`connected` 同步翻转/`ERR_IPC_DISCONNECTED`）+ primary 侧 disconnect→exit 序列 | IDENTICAL |
| `p3` | worker 侧 `cluster.worker.on('message')`/`send`/`listenerCount`/`isConnected`/`isDead` | IDENTICAL |
| `p4` | `process` 事件器值语义（14 条断言：返回值/别名同一性/once 自删/listeners 副本） | IDENTICAL |
| `p5` | worker 通道保活（300ms 处 `connected=true`、`state=online`） | IDENTICAL |

**门禁 e2e**：`cargo test -p aluka-cli --all-features --test m52_worker_msg_test` →
`5 passed; 0 failed`，`--nocapture` 确认**无 `[SKIP node-e2e]`**（真对拍）。

**既有 M5 用例回归**：`m52_cluster_events_test`(4) / `m52_settings_test`(7) /
`m52_http_cluster_test`(3) / `builtins_phase6_proc_test`(11) / `m5_semantics_test`(8) /
`builtins_phase9_m4_test`(1) / `express_e2e_test`(1) 全绿；
M5 差分门禁 `ALUKA_CONF_FILTER=m5` → **5/5 PASS, 0 invalid**。

### 10.5 门禁三连（真实输出）与交付摘要

```text
# 1. 格式化门禁
$ cargo fmt --all --check                      FMT_CHECK_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ cargo clippy --all-targets --all-features -- -D warnings   CLIPPY_EXIT=0

# 3. 全工作区全量测试门禁（NODE=<v22.23.1>，CARGO_INCREMENTAL=0）
$ cargo test --workspace --all-features        TEST_EXIT=0   （墙钟 229.9s）
聚合：suites=89 passed=629 failed=0 ignored=1
      grep -c '^test result: FAILED' = 0 ；grep -c '^error' = 0 ；
      grep -c '[SKIP node-e2e]' = 0（新增用例全部真对拍）
```

- 与上一轮基线（§9.6：88 suites / 624 passed / 0 failed / 1 ignored）对照：
  **+1 套件、+5 用例**，恰好等于本轮新增的 `m52_worker_msg_test.rs`；
  `1 ignored` 为既有 doc-test（`builtins::builtin_module`），非本轮引入。
- 本轮 Clippy 首跑命中 `clippy::iter_over_slice`（`args.iter().copied().collect()`）
  → 改 `args.to_vec()` 后零告警（记录以便复用）。

**本轮新登记的偏离（诚实登记，不静默）**：

| 项 | Node | 本运行时 | 影响 |
|---|---|---|---|
| channel ref 粒度 | `refCounted`/`unrefCounted` 按 `'message'`/`'disconnect'` 监听器**计数** ref，`process.channel.ref()/unref()` 可显式干预 | 以「通道连通即保活」等价实现 | 显式 `process.channel.ref/unref`、计数式 unref 未接线（可观测面：Node 在移除全部监听器后仍保活，本运行时行为等价于默认保活） |
| worker 侧 `cluster.worker.disconnect()` / `isDisconnected` | `Worker.prototype.disconnect` 置 `'disconnecting'` 并发 `{act:'disconnect'}` 帧 | 不实现（worker 内可用 `process.disconnect()` 达成同一语义） | 该方法缺失 |
| primary 发起的 `cluster.disconnect()` | 下发 `{act:'disconnect'}` 帧，worker 走 `_disconnect(true)` 并以 `exitedAfterDisconnect=true` 退出 | 仍走既有 `destroy`（杀进程）路径 | 沿用 §9.7 登记（`exitedAfterDisconnect` 恒 false） |
| worker 侧 `cluster.worker.isDead()` | `process.exitCode/SignalCode != null` | 恒 `false`（二者未接线；进程存活时二者恒假值，故等价） | 仅退出路径不可观测 |
| 被杀子进程的 `console.log` 缓冲 | 继承 stdio 逐写直落，被杀前的输出仍可见 | `console.log` 走行模型缓冲（`vm.stdout_records`，CLI 运行结束后统一输出），被 `kill()` 时缓冲丢失 | **引擎级既有行为**（非本轮引入）；探针纪律新增第 4 条 |
| worker 内未 `require('cluster')` 时的 `listenerCount('message')` | 0（桥接仅在 require 时挂接；但通道仍默认保活） | 本运行时桥接同样按 require 挂接 → 0 ✔（保活由通道连通承担） | 无（已对齐） |

**未处理项（本轮不擅自扩张）**：`process.channel`（`ref`/`unref`/`hasRef`/`fd`）对象面、
`process.on('internalMessage')`、`process.on('newListener')`/`'removeListener'` 元事件。

### 10.6 项 1（真 RR 调度）的处置（登记，未实施）

`cluster.schedulingPolicy` 恒 `SCHED_NONE`、端口由内核（`SO_REUSEADDR`/`REUSEPORT`）
分发。Node 的 `SCHED_RR` 需要 **primary 自己 accept 连接后把 server 句柄经 IPC 传给
被选中的 worker**（`round_robin_handle.js` + `sendHelper(..., handle)`）：这要求
1）primary `listen` 并 accept；2）句柄（socket）跨进程传递；3）primary 侧轮询选 worker。
本运行时的 IPC 通道为**回环 TCP 行协议**（无句柄传递，`cluster_ipc` 文档已登记
「句柄传递未实现」），且端口共享策略本身是内核分发——改造属架构级变更（影响
`net.rs`/`http/server.rs` 的 listen 路径与 Windows 无 REUSEPORT 的回退分支），
超出 M5.2 收口范围。**建议**：单独立项评估（与 `process.channel`/句柄传递面一并），
本轮仅保留登记，不放宽任何断言。

### 10.7 提交证据

```text
$ git commit -F -   # feat(m5.2): worker 侧 IPC 面——process.on('message') 接收、process.disconnect() 与通道默认保活
[master eee4114] 10 files changed, 1223 insertions(+), 96 deletions(-)
 create mode 100644 crates/aluka-cli/tests/m52_worker_msg_test.rs
```

只暂存目标文件（`.work/scratch/`、`.workbuddy/` 未入库）。改动文件：
`proc_common.rs` / `require_aliases.rs` / `cluster_ipc.rs` / `cluster.rs` /
`interpreter.rs` / `modules.rs` / `m52_worker_msg_test.rs`（新增）/
`.work/TODO/README.md` / `.work/TODO/20260911/README.md` / `docs/builtins-manifest.md`。

**门禁复跑口径（重要）**：本轮门禁以 `NODE=C:\Users\User\AppData\Local\pi-node\current\node.exe`
（`node --version` = **v22.23.1**）执行——本机 `PATH` 上的 `node` 为 nvmd 的
**v22.3.0**，不设 `NODE` 时会以较低版本对拍（与本仓声明的权威 Oracle 不符）。
建议后续门禁固定 `NODE` 指向 v22.23.1（或把该版本放入 `PATH`）。

**提交后剩余 M5.2 缺口**：真 round-robin 调度（§10.6 登记处置建议）、
`process.channel`（`ref`/`unref`/`hasRef`/`fd`）对象面、worker 侧
`cluster.worker.disconnect()`、primary `cluster.disconnect()` 的 `{act:'disconnect'}`
帧路径（§10.5 偏离表）。M5 整体仍为 `[~]`（M5.1 余 `postMessageToThread` 真线程
分支 / eval worker；M5.3 余 ctor options / 真预编译；M5.4 余 LCOV / 真
`stream.Transform` 报告器）。

---

## 11. 待办 29 · M5.2 剩余项⑤⑥⑦⑧（worker/primary 断连闭合 + `process.channel` + RR 决策）

> 触发指令：用户点名四项剩余缺口——「真 RR 调度（架构级）、`process.channel`
> （`ref`/`unref`/`hasRef`/`fd`）对象面、worker 侧 `cluster.worker.disconnect()`、
> primary `cluster.disconnect()` 的 `{act:'disconnect'}` 帧路径」。

### 11.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | worker 侧 `cluster.worker.disconnect([cb])` | 按 Node 实测语义：返回 `cluster.worker` 自身、worker 侧 `state='disconnecting'` 与 `exitedAfterDisconnect=true` **同步**置位、关闭 worker 内 server、优雅退出码、primary 侧事件序与 `exitedAfterDisconnect` | §11.3/§11.4 |
| 2 | primary 侧 `cluster.disconnect([cb])` 走 `{"t":"d"}` 帧路径 | 不再以 `destroy`（杀进程）收尾：worker 优雅退出、primary 侧 `exitedAfterDisconnect===true`、`cluster.workers` 立刻清空、`cb` 在全部 worker 出表后触发且早于 `worker.on('exit')`；worker 内活跃 server 场景亦能退出 | §11.3/§11.4 |
| 3 | `process.channel` 对象面（worker 侧） | 面型与方法面按 Node 实测；`unref()` 真正解除保活（排空后自然退出 code 0），`ref()` 可恢复 | ✅ §11.4 |
| 4 | 真 RR 调度（`SCHED_RR`） | 要么实现并对拍通过；要么给出**可核验的不可行结论**（Node 侧可观测性实测 + 仓库约束证据），登记为架构级偏离且不放宽断言 | ✅ §11.5 |
| 5 | 既有用例不回归 | `m52_worker_msg_test`(6) / `m52_cluster_events_test`(4) / `m52_settings_test`(7) / `m52_http_cluster_test`(3) / `builtins_phase6_proc_test`(11) / `phase9_m4`(1) / `express_e2e`(1) + M5 差分门禁 5/5 + 门禁三连全绿 | §11.7 |

**红线**：Node 22 唯一权威；做不到的如实登记，不放宽断言凑绿；`cluster.disconnect` 语义
变更必须同步复核既有「本地锚点」用例（`builtins_phase6_proc_test.rs`）并写明 Node 依据；
本轮**主动收窄范围**（项 1/2 见 §11.6，不以半成品入库）。

### 11.2 Oracle 取证（两路独立后台委托）

并行两路：① `process.channel` 与 primary/worker 两侧断连语义；② RR 调度可行性 +
server 注册面。**结论见 §11.3**。
### 11.3 Oracle 结论（两路独立取证，node v22.23.1，连跑 3~4 次稳定）

**取证方式**：两个独立后台委托——① `process.channel` / 断连语义；② RR 调度可行性 +
server 注册面。全部探针与原始输出在两份会话 scratch（`channelsem/`、`rrfeas/`），
结论如下（未测到项已显式标注）。

**A. `process.channel`（worker 侧）**

| 语义点 | Node 实测 |
|---|---|
| 存在性 | worker 内 `typeof process.channel === 'object'`（构造类名 `Control`）；primary 内 `typeof` 为 `undefined` 且非自有属性 |
| 方法面 | `Control.prototype`：`ref`/`unref`/`refCounted`/`unrefCounted`（函数）、`fd`（getter，值 **3**）；**无 `hasRef`**（`undefined`）；`_handle` 为 `undefined` |
| 返回值 | `ref()` / `unref()` 均返回 `undefined` |
| 默认保活 | worker 脚本跑完**不退出**（>14s）；无其它句柄时 `channel.unref()` 后**下一个 tick 即以 code 0 自然退出**（`process.connected` 退出瞬间仍为 `true`；primary 先收 `'disconnect'` 再收 `'exit'`） |
| 可逆性 | `unref()`→`ref()`（同 tick 或晚 tick）均可恢复保活 |

**B/C. 断连语义（primary `cluster.disconnect()` / worker `cluster.worker.disconnect()`）**

| 语义点 | Node 实测 |
|---|---|
| `cluster.worker.disconnect()` 返回值 | **返回 `cluster.worker` 自身**（同一 Worker 对象，非 undefined/Promise） |
| worker 侧 state / exitedAfterDisconnect | 调用**同步**置 `state='disconnecting'`、`exitedAfterDisconnect=true`，此后保持（不会变 `'disconnected'`/`'dead'`） |
| primary 侧 `exitedAfterDisconnect` | `cluster.disconnect()` / `worker.disconnect()`（两侧发起）→ **`true`**；worker 自己调 **`process.disconnect()`** → **`false`**（区分「谁断的连」的唯一痕迹） |
| primary 侧事件序 | `worker 'disconnect'`(0 实参) → `cluster 'disconnect'`(1 实参) → `'exit'`；`state` `'disconnected'`→`'dead'` |
| `cluster.workers` 表 | primary 发起断连：**立刻清空**；worker 自己发起：`'disconnect'` 时**仍在表内**，到 `'exit'` 才清空 |
| `cluster.disconnect([cb])` | 返回 `undefined`；`cb` 无实参、在**全部 worker 出表后**触发且**早于** `worker.on('exit')`；primary 之后能自然退出（code 0） |
| worker 内活跃 server | 上述两种断连都会**关闭 worker 内 `listen()` 的 server 并触发 `'close'`**（`listening=false`）——worker 自己调 `disconnect()` 时该关闭是**同步**的（同 tick、`setImmediate` 之前）；否则 worker 因 server 句柄无法退出 |
| 通道关闭后 `send()` | 同步返回 `false`，随后在 `process` 与 `cluster.worker` 上各发一个 `'error'`（`code='ERR_IPC_CHANNEL_CLOSED'`）；无监听器则崩溃、退出码 1 |

**未测到 / 不稳定（不据此下结论）**：primary 侧 `worker.state === 'disconnecting'` 的瞬时态
（Windows 定时器粒度下观测不到，worker 侧稳定可见）；`channel` 的符号键私有属性与
`refCounted` 多次配对的计数语义；`cluster.disconnect()` 在 primary 自身也 listen 时的退出。

### 11.4 交付摘要与对拍证据（本轮落地项 3；项 1/2 见 §11.6）

**项 3 · `process.channel` 已落地**（源码 3 文件 + 1 用例）：

| 文件 | 改动 |
|---|---|
| `builtins/cluster_ipc.rs` | 子进程侧状态扩为 `refed`/`refs`/`explicit`（默认 `refed=true` = Node 实测的默认保活）+ 5 个开关原语（`child_channel_ref/unref/ref_counted/unref_counted/refed`） |
| `builtins/cluster.rs` | `process.channel` 单例对象（`cluster:channel` 命名空间 + `fd` 自有键）+ 4 个处理器 + `sync_worker_ipc_source`；`cluster_ipc_busy()` 的 worker 分支改为「连通 **且** 未被 unref」 |
| `interpreter.rs` | worker 内挂 `process.channel` 属性（primary 侧不挂，与 Node 一致） |
| `tests/m52_worker_msg_test.rs` | 新增用例 6 `cluster_worker_process_channel_matches_node` |

**对拍证据（逐字节）**：新探针 p6 —— Node 连跑 3 次、aluka 连跑 3 次，两侧输出
**完全一致**（`IDENTICAL=True`）：

```text
P online / P send-intro=true
P intro typeof=object fd-in=true hasRef=undefined fns=function,function,function,function
P send-go=true / P unrefref r1=undefined r2=undefined connected=true
P send-final=true / P final r3=undefined connected=true
P exit code=0 signal=null
```

判别力：`unref()` 若未真正解除保活，末次 `unref` 后 worker 不会退出（用例会因缺
`P exit code=0` 失败）；若 `unref` 生效过早（跨 tick 未恢复），`unrefref` 之后的
`final` 回复收不到（用例会在门控处失败）。

**门禁 e2e**：`cargo test -p aluka-cli --all-features --test m52_worker_msg_test` →
**6 passed / 0 failed**（`--nocapture` 无 `[SKIP node-e2e]`）。

**本轮新登记的偏离**：`process.channel.fd` 值（Node 为 IPC 管道 fd `3`；本运行时介质为
回环 TCP，仅提供同名自有键、值为 `undefined`——`'fd' in process.channel` 两侧同为真）；
`channel.constructor.name`（Node `Control`）与 channel 自身的 EventEmitter 方法面
（`on`/`once`/`emit`…）、`_handle` 未接线。

### 11.5 RR 调度决策记录（项 4：判定「需 unsafe FFI，与仓库策略冲突」，不实现）

**判定：真 `SCHED_RR` 在本仓当前约束下不可落地**，三层阻塞均有可复核证据：

1. **机制要求跨进程句柄传递**：Node `SCHED_RR` = primary `listen` + accept，再把 socket
   句柄经 IPC 交给轮询选中的 worker（`round_robin_handle.js`）。本运行时的 IPC 介质是
   **回环 TCP 行协议**（`cluster_ipc.rs:5-8,16-19`），模块文档早已登记「句柄（socket/
   server handle）传递未实现」（`cluster_ipc.rs:36-37`）；Unix 侧即使换介质也需 Unix
   domain socket + SCM_RIGHTS，属介质层架构改造。
2. **唯一可用的 Win32 路径必为 `unsafe`，与仓库策略冲突**：跨进程 socket 复制只能走
   `WSADuplicateSocketW` + 子进程 `WSASocketW`；socket2 0.5.10 的 `try_clone()`
   Windows 实现（`socket2/src/sys/windows.rs:390-413`）只做**同进程**复制，未暴露
   `WSAPROTOCOL_INFOW` 传递；把 raw socket 装回 `std::net` 类型同样是 `unsafe`。而
   workspace 级策略是 `unsafe_code = "deny"`（`Cargo.toml:42-44`），全仓仅
   `aluka-jit`（`Cargo.toml:25`）与 `aluka-vm` 的 `jit_helpers.rs:15`/`jit_hot.rs:15`
   显式解禁；`builtins/` 下**零解禁**。走 RR 需在 `builtins` 新增 unsafe 例外，
   与 AGENTS.md「unsafe 默认全仓禁用」冲突。
3. **需新增直连依赖并触碰 listen/accept 主路径**：`winapi` 不在依赖图内；`windows-sys`
   仅有传递依赖（0.52/0.59），任何 `Cargo.toml` 均未直连声明。改造还需动
   `net.rs`/`http/server.rs` 的 listen 路径与 Windows 无 `SO_REUSEPORT` 的回退分支
   （现状 `net.rs:1508-1536` 的 `bind_shared_listener` 即 SCHED_NONE 的实现面）。

**Node 侧可观测性实测（说明「为什么这是真实差距」而非文档措辞）**：primary fork 2 worker、
同一端口、8 次串行请求命中序列——`SCHED_RR`：`w1,w2,w1,w2,w1,w2,w1,w2`（严格交替，
3/3 一致）；`SCHED_NONE`：`w2,w2,w2,w2,w2,w2,w2,w2`（全落一个 worker，3/3 一致）。
两者每次请求命中均可区分（但 `server.address()`/`listening`/netstat 拓扑在两侧相同，
故判据只能取请求序列）。即本运行时当前行为**语义对应 SCHED_NONE**（内核分发），
与 `SCHED_RR` 的差距是真实且可测量的。

**替代方案（本轮建议，未实施，等 owner 决策）**：
1. 维持内核分发，把差距如实登记为架构级偏离（本轮已登记）；
2. 若希望「设置 `schedulingPolicy` 不静默失效」，最小改造是把 `schedulingPolicy` 做成
   Node 形态的访问器（含「已有 worker 时再设置报错」）——**需先取 Node 该行为的 oracle**，
   本轮未做，不擅自实现；
3. 不建议「primary 中继字节流」的伪 RR：worker 侧 net/http 泵完全基于自身
   `TcpListener`/`TcpStream`，中继需重写连接接入面，成本高于收益且仍不满足 `sendHandle` 语义。

### 11.6 项 1/2（断连切片）本轮未实施的原因与已备条件

**未实施原因（诚实登记）**：项 1/2 需要「worker 断连时关闭其内 server」这一前置能力，
且会改动 `cluster.disconnect()` 的既有语义（回调时机、`workers` 表清空时机、primary
存活性判定）——属一次独立的完整切片，本轮在完成项 3 与项 4 决策后**主动收窄范围**，
不以半成品入库。

**已备条件（下一轮可直接开工）**：
* **oracle 已取全**（§11.3 表 B/C，含返回值/同步迁移/事件序/两处 `exitedAfterDisconnect`
  差异/`workers` 表差异/server 同步关闭/callback 时机）；
* **前置件规格**（server 批量关闭）：`net` 侧在 `NET_SHARED.servers`
  （`net.rs:114-127,146-148`）上遍历未 closed 项、按 `net_server_close`
  （`net.rs:1102-1146`）语义置 `closed=true`/`listener=None` 并把 `'close'` 入队、
  末尾 `activate_event_source("net", net_pump)`；`http` 侧在 `SERVERS`
  （`http/state.rs:38-53,169-170,246-256`）上镜像 `server_close`（`http/server.rs:283-309`：
  `listening=false`/`listener=None`/`conns.clear()`）+ `sync_event_source`
  （`http/mod.rs:85-97`）。**注意** `server_close_all`（`http/server.rs:361-364`）是显式
  no-op，不要改它；http 的 socket **不在** `NET_SHARED` 里，两表都要扫；
* **一份已写好但**（因本轮不落地而）**未入库**的实现草稿**存于会话 scratch
  （`sweep-uncommitted.patch`，99 行，已 `git checkout` 回退以保持树绿）；
* 协议帧位已预留：`{"t":"d"}`（primary 发起断连）与 `{"t":"e"}`（exitedAfterDisconnect
  ack）在 `cluster_ipc` 模块文档中登记但尚无发送方。


### 11.7 门禁三连（真实输出）

```text
# 1. 格式化门禁
$ cargo fmt --all --check                        FMT_CHECK_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ cargo clippy --all-targets --all-features -- -D warnings   CLIPPY_EXIT=0

# 3. 全工作区全量测试门禁（NODE=<v22.23.1>，CARGO_INCREMENTAL=0）
$ cargo test --workspace --all-features          TEST_EXIT=0   （墙钟 262.6s）
聚合：suites=89 passed=630 failed=0 ignored=1
      grep -c '^test result: FAILED' = 0 ；grep -c '^error' = 0 ；
      grep -c '[SKIP node-e2e]' = 0（新增/存量对拍用例全部真跑）
```

- 与上一轮基线（§10.5：89 suites / 629 passed / 0 failed / 1 ignored）对照：
  **+0 套件、+1 用例**，恰好等于本轮新增的 `cluster_worker_process_channel_matches_node`；
  `1 ignored` 为既有 doc-test，非本轮引入。
- 本轮 Clippy 首跑命中 `unused_doc_comments`（`thread_local!` 宏前的 `///`）→ 改 `//` 后零告警。

### 11.8 提交证据
```text
$ git commit -F -   # feat(m5.2): worker 侧 process.channel 对象面（ref/unref 保活开关）+ RR 调度决策记录
[master 61da226] 5 files changed, 471 insertions(+), 4 deletions(-)
```

只暂存目标文件（`.work/scratch/`、`.workbuddy/` 未入库）。改动文件：
`cluster_ipc.rs` / `cluster.rs` / `interpreter.rs` / `m52_worker_msg_test.rs` /
`.work/TODO/20260911/README.md`。

> **范围说明（诚实登记）**：用户点名四项中，本轮落地**项 3（`process.channel`）**并
> 给出**项 4（RR）的可核验决策记录**；**项 1/2（worker/primary 断连切片）本轮未实施**
> ——其 oracle 与前置件规格已备全（§11.6），未以半成品入库。并行委托写好的
> `net`/`http` 批量关闭实现草稿（99 行）已存档于会话 scratch 并 `git checkout` 回退，
> 以保证本轮树绿（避免 `dead_code` 与 `-D warnings` 冲突）。

---

## 12. 断连切片落地（项 1/2 续做）与新发现的引擎级缺陷

### 12.1 落地结果（提交见 §12.4）

§11.6 登记的「下一轮可直接开工」在本轮完成：

| 改动 | 内容 |
|---|---|
| `builtins/cluster.rs` | primary 侧：`intercom`（`once`/`emit` 语义）+ `remove_worker`（表空即 `emit`）+ primary `Worker.prototype.disconnect`（置 `ead=true` → 发 `{"t":"d"}` 帧 → 立即出表 → 返回 `this`）+ `cluster.disconnect(cb)` 重写（不再 `destroy` 杀进程；workers 为空走 `nextTick`）+ worker 侧 `cluster.worker.disconnect()`（返回自身、同步置 `'disconnecting'`/`ead=true`、关本进程 server、再 `process.disconnect()`）+ `{"t":"d"}`/`{"t":"e"}` 帧处理 |
| `builtins/cluster_ipc.rs` | 帧协议文档补齐（`d` = primary→worker 断连；`e` = worker→primary 先行上报；Node 的 primary→worker ack 回程未实现，已登记） |
| `builtins/net.rs` / `builtins/http/{mod,server}.rs` | 新增 `pub(crate)` 批量关闭函数（断连时关闭 worker 内**全部监听中的 server**，对齐 Node：`cluster.worker.disconnect()` 会同步关闭 worker 内 server，否则 worker 无法优雅退出） |
| `gc.rs` | `cluster::store_roots`（`intercom` 待发回调的 GC 根） |
| `tests/m52_disconnect_test.rs`（新增） | 2 例 `assert_e2e_matches_node` 真对拍 |

**对拍证据（逐字节）**：探针 p7（primary `cluster.disconnect`）与 p8（worker
`cluster.worker.disconnect`）在 Node v22.23.1 连跑 3 次、aluka 连跑 3 次，**两侧输出
完全一致**（`IDENTICAL=True`）：

```text
[p7] W listening id=1 listening=true / W close id=1 / P online id=1 state=online /
     P before-disconnect workers=1 / P disconnect-ret=undefined workers-after=0 /
     P w-disconnect id=1 argc=0 state=disconnected ead=true workers=0 /
     P c-disconnect id=1 argc=1 / P cb argc=0 workers=0 /
     P w-exit id=1 code=0 signal=null state=dead workers=0
[p8] W before state=listening ead=undefined listening=true /
     W ret-is-self=true ret-type=object /
     W after state=disconnecting ead=true listening=false / W close /
     P online state=online /
     P w-disconnect argc=0 state=disconnected ead=true workers=1 /
     P c-disconnect id=1 argc=1 / P w-exit code=0 signal=null state=dead workers=0
```

**回归**：`m52_worker_msg_test`(6) / `m52_cluster_events_test`(4) / `m52_settings_test`(7) /
`m52_http_cluster_test`(3) / `builtins_phase6_proc_test`(11，含 `w.kill()` + `cluster.disconnect(cb)`
的 `disconnected cb` 断言) / `builtins_phase9_m4_test`(1) / `express_e2e_test`(1) 全绿；
M5 差分门禁 `5/5 PASS, 0 invalid`；`fmt` / `clippy -D warnings` 均 0。

**本轮新登记的偏离**：`{"t":"e"}` 的 primary→worker **ack 回程**未实现（worker 上报后
即本地断连；Node 会等 ack 再 `process.disconnect()`）；`intercom` 为最小实现
（仅 `once`/`emit` 两个用途，非完整 EventEmitter）。

### 12.2 新发现的引擎级缺陷（⚠️ 独立专项，本轮只登记不修）

**现象**：**块内函数声明**在 aluka 侧**完全不可见**（块内 `typeof` 也是 `undefined`）。

```js
if (true) { function inner() { return 'inner-ok'; } console.log(typeof inner); }
// Node  v22.23.1: "function"     （块内可调用）
// aluka          : "undefined"    ← 缺陷
```

| 形态 | Node 实测 | aluka 实测 |
|---|---|---|
| `if` 块内 `function` 声明的块内可见性 | `function`（可调用） | **`undefined`** |
| 普通块（`{ … }`）内 | `function` | **`undefined`** |
| 块外（sloppy 模式） | `undefined`（两边一致） | `undefined` |
| 函数**表达式**赋值（`const f = function () {}`） | `function` | `function`（正常） |

**来源**：本轮实现断连切片时，探针首版把 `start` 写成 `if (cluster.isPrimary) { function
start() {…} … }`——Node 侧正常、aluka 侧 `w.on('message')` 内调 `start()` 抛
`TypeError: undefined is not a function`（primary 直接崩，输出仅剩 worker 首行）。用
「每个处理器独立 try/catch + 逐步打印」的探针定位到精确抛点后，再用最小复现
（`p9.js`）确认是**作用域绑定**问题而非本次改动引入。

**影响面（重要）**：所有把辅助函数声明写在 `if`/`for`/普通块内的真实 JS 代码在 aluka
上会失败（真实包中常见）。属**解析/编译期作用域绑定**专项，跨 M5 范围。

**处置**：① 探针纪律新增第 5 条——跨引擎探针一律用函数表达式（已写入
`m52_disconnect_test.rs` 文件头与 `20260911/README.md` §11.2 纪律）；② 最小复现用例
已放入门禁**隔离区** `tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0001.cjs`
（该目录结构性不参与门禁，见既有 `DEVIATIONS.md`）；③ 缺陷本体**本轮不修**（需改
parser/compiler 的块级函数声明绑定，属独立专项），已在 TODO 总表登记。

### 12.3 四项缺口最终状态

| # | 项 | 状态 |
|---|---|---|
| 1 | worker 侧 `cluster.worker.disconnect()` | ✅ 已闭环（§12.1） |
| 2 | primary `cluster.disconnect()` 的 `{"t":"d"}` 帧路径 | ✅ 已闭环（§12.1） |
| 3 | `process.channel`（`ref`/`unref`/`hasRef`/`fd`） | ✅ 已闭环（§11.4；`fd` 值、`Control` 类名为登记偏离） |
| 4 | 真 RR 调度 | ⛔ 判定「需 unsafe FFI + 换 IPC 介质 + 新直连依赖」，与仓库 `unsafe_code=deny` 策略冲突 → 登记为**架构级偏离**（§11.5），替代方案待 owner 决策 |

**另**：RR 相关的 `schedulingPolicy` 目前是普通数据属性（可写可读、无调度效果）。若要求
「设置后不静默失效 / 与 Node 一致的访问器行为」，需先取 Node 该行为的 oracle（本轮未做）。

### 12.4 提交证据
### 12.5 门禁三连（真实输出）

```text
# 1. 格式化门禁
$ cargo fmt --all --check                                          FMT_CHECK_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ cargo clippy --all-targets --all-features -- -D warnings         CLIPPY_EXIT=0

# 3. 全工作区全量测试门禁（NODE=<v22.23.1>，CARGO_INCREMENTAL=0）
$ cargo test --workspace --all-features -j 4                       TEST_EXIT=0（墙钟 190.3s）
聚合：suites=90 passed=632 failed=0 ignored=1
      grep -c '^test result: FAILED' = 0 ；grep -c '^error' = 0 ；
      grep -c '[SKIP node-e2e]' = 0（新增/存量对拍用例全部真跑）
```

- 与上一轮基线（§11.7：89 suites / 630 passed / 0 failed / 1 ignored）对照：
  **+1 套件、+2 用例**，恰好等于本轮新增的 `m52_disconnect_test.rs`（2 例）；
  `1 ignored` 为既有 doc-test，非本轮引入。
- **工程隐患（本轮新增）**：首次以默认并发跑全量测试时 `link.exe` 报
  `exit code 1171`（Windows 链接器资源不足/句柄压力，非代码问题），`-j 4` 重跑即过。
  建议门禁在 Windows 上固定并发上限（`-j 4`）或串行链接。

### 12.4 提交证据

```text
$ git commit -F -   # feat(m5.2): cluster 断连闭环——primary {act:'disconnect'} 帧路径 + worker cluster.worker.disconnect()
[master 7012ce0] 9 files changed, 816 insertions(+), 39 deletions(-)
 create mode 100644 crates/aluka-cli/tests/m52_disconnect_test.rs
 create mode 100644 tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0001.cjs
```

只暂存目标文件（`.work/scratch/`、`.workbuddy/` 未入库）。改动文件：
`cluster.rs` / `cluster_ipc.rs` / `net.rs` / `http/mod.rs` / `http/server.rs` /
`gc.rs` / `m52_disconnect_test.rs`（新增）/ `gen-block-fn-decl-0001.cjs`（新增，
门禁隔离区）/ `.work/TODO/20260911/README.md`。

**过程登记（诚实记录）**：本切片的实现由一次后台委托完成，该委托在提交前撞轮上限，
留下两处编译错误（`ns_attach` 未闭合括号、`Option<Vec<Value>>` 未解包）——我定位并修复
后，其实现经 p7/p8 逐字节对拍与全部回归验证通过。定位过程中的一次误判也记录在案：
primary 侧 `start` 崩溃曾怀疑为实现缺陷，实测为**块内函数声明**引擎缺陷（§12.2）。

**文档工具链教训（本轮）**：本文件的一次「用 PowerShell `Set-Content` 做全文替换」
导致编码/行数被破坏（1211 行 → 864 行）——已用 `git checkout` 恢复并改用编辑工具重做。
**口径**：本仓中文文档一律用编辑工具（Edit/Write）修改，不用 shell 文本替换。

---

## 13. 引擎缺陷修复：块内函数声明提升（§12.2 的处置）

### 13.1 根因与修法

**根因（两处叠加）**：
1. `codegen.rs` 的 `Stmt::Function(_) => { … }` 是**空实现**（只保证栈平衡，注释写
   「在 compile_module 中提取」）；
2. `module.rs` 的「提升收集」只遍历**直接子语句**（`for stmt in def.body.iter()` /
   `optimized_program.body.iter()`）——`if`／`for`／普通块内的 `Stmt::Function` 既不被
   提取，也不被绑定 → `typeof` 恒为 `undefined`。

**修法**（`crates/aluka-compiler/src/module.rs`，3 处改动）：
1. 新增 **递归收集** `collect_scope_functions` / `collect_scope_functions_in_stmt`：
   进入 `Block`／`if`／`while`／`do-while`／`for`／`for-in`／`for-of`／`try`／`switch`／
   `export`，**不进入**嵌套函数的函数体（那是子函数自己的提升域），不进入表达式
   （函数表达式非声明）；
2. 模块顶层与函数体的提升收集改为调用该递归收集；`ordered`（非提升语句序列）保持
   只含直接子语句（块语句本身仍在原位编译）；
3. 顶层与函数体的**绑定预注册**各补一段：对递归收集到的函数名 `ensure_slot`，
   保证提升编译时的 `ParentScopeInfo` 快照与上值捕获识别都能看到这些名字。

模块文档同步（`collect_scope_functions` 的 doc comment 写明语义依据、递归边界与
Node 实测形态）。

### 13.2 验证（red → green）

**最小复现对比**（`p9.js`，两侧同源）：

| 断言 | 修复前 aluka | Node v22.23.1 | 修复后 aluka |
|---|---|---|---|
| `if` 块内 `typeof f` | `undefined` | `function` | ✅ `function` |
| `if` 块内调用 `f()` | 抛 TypeError | `inner-ok` | ✅ `inner-ok` |
| 普通块内 `typeof f` | `undefined` | `function` | ✅ `function` |
| 块外 `typeof f`（块执行后） | `undefined` | `function` | ✅ `function` |
| 块**执行前** `typeof f` | `undefined` | `undefined` | ⚠️ `function`（余差异，见 §13.3） |

**回归保护用例**（新增，**在门禁内**与 Node 逐字节对拍）：
`tests/conformance/node22/cases/gen/gen-block-fn-decl-0002.cjs` —— 覆盖普通块（含块内
**声明之前**调用）、`if` 块、`for` 块、`try` 块、函数体内块、以及「块内函数捕获外层
变量」的上值路径（`upvalue=3`）。两侧输出**逐字节一致**：

```text
plain-block-in=function call=inner-ok / plain-block-after=function
if-block-in=function call=if-ok       / if-block-after=function
loop-in=function call=loop-ok         / try-in=function call=try-ok
fn-body-blocks=function,function      / upvalue=3
```

门禁过滤验证：`ALUKA_CONF_FILTER=block-fn-decl` → `PASS gen/gen-block-fn-decl-0002.cjs`、
`Result: 1/1 passed, 0 invalid`。

**影响面评估（本轮修复的价值）**：修复前该缺陷会让任何「把辅助函数声明写在 `if`/`for`/
普通块内」的真实代码直接抛 `TypeError`（本轮 M5.2 断连探针首版即因此崩溃、并一度被
误判为功能实现缺陷）。修复后该类代码可用。

### 13.3 余差异（如实登记，不静默）

| 项 | Node | 本运行时 | 影响 |
|---|---|---|---|
| 块内函数声明在**块执行前**的值 | `undefined`（绑定在函数入口初始化为 undefined，块执行时才赋值） | `function`（提升编译在函数入口即 `MakeClosure` + `StoreLocal`） | 仅「块执行前引用该名字」可见（如函数入口特性探测）；方向为**更宽松**，不崩溃 |
| 块内函数声明捕获**同块 `let`/`const`** | 正常（函数与块级绑定同处块作用域） | 读到 `undefined`（提升函数捕获函数级预注册槽，而块级 `let`/`const` 写入块级槽） | **功能缺口**：`{ const rec = []; function f() { rec.push(1); } f(); }` 抛 `TypeError`；见 §13.6 |

**已隔离**：`tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0001.cjs`
（该目录结构性不参与门禁）。

**精确对齐的修法（已勘察，未实施）**：把绑定动作从函数入口**下移到块入口**——块内
函数模板在编译期预编译后入队（按收集序），`codegen.rs` 的 `Stmt::Block` 分支在编译
子语句前取出对应模板并 `MakeClosure` + `StoreLocal`。需保证「块编译序 == 收集序」
（当前两者同为先序），属编译期绑定时机专项，风险中等，建议独立一轮施行并配
deviations 用例回填。

### 13.6 二次修复尝试与回退（块内函数捕获同块 `let`/`const`）

### 13.6.1 现象

修复「块内函数可见性」（§13.1）后暴露的新缺口——**块内函数捕获同块的 `let`/`const`**：

```js
if (true) {
  const rec = [];
  function start() { rec.push('ok'); return rec.length; }   // 提升到函数作用域
  start();                                                  // 修复前：rec 为 undefined
}
// Node v22.23.1: same-block-capture=1
// aluka（当前）  : TypeError: Cannot read properties of undefined (reading 'push')
```

**根因（编译期槽位模型，三处叠加）**：
1. 块内函数声明被提升到**函数作用域** → 其闭包在**函数入口**创建，捕获函数级预注册槽；
2. 块级 `let`/`const` 在 `codegen.rs` 的 `block_depth > 0` 分支中**总是分配新槽**
   （用于实现块级遮蔽），值写入**块级槽**；
3. 两者不是同一个槽 → 提升函数读到函数级槽的 `undefined`。

### 13.6.2 已尝试的方案与回退原因（重要记录）

| 方案 | 做法 | 结果 |
|---|---|---|
| 递归预注册块内绑定名 | 让块内函数名与其同块 `const` 在提升编译前入 `symbol_map` | 不足（槽仍非同一个） |
| 块级 `let`/`const` **复用**函数级预注册槽 | 新增 `nested_preregistered` 集合 + `codegen.rs` 两处判据（仅当无 shadow 记录时复用） | ❌ **破坏块级遮蔽**：`(() => { let x = 10; { let x = 20; } return x })()` 实测 Node=`10` / aluka=`20`（`gen-lang-more-0014.cjs`）→ **已 `git checkout` 回退** |

**结论**：近似方案不可取（遮蔽是更高频语义）。**当前保持 §13.1 的净改善**（块内函数
可见性），把「同块 `let`/`const` 捕获」如实登记为**未修缺口**，并隔离用例：

```
tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0002.cjs
```

### 13.6.3 精确修法（已勘察，未实施）

把绑定动作从函数入口**下移到块入口**：块内函数模板在编译期预编译并登记（按收集序，
或按「名字 + 出现序」匹配），`codegen.rs` 的 `Stmt::Block` 分支在编译子语句前取出
模板并 `MakeClosure` + `StoreLocal`——此时闭包与块级 `let`/`const` 同处块级槽位语义，
遮蔽语义亦不受影响。

需注意的前置：`compile_stmt` 是自由函数（无 `self`），故模板必须**预先编译**并随
`CompiledUnit` 传递（新增字段），块分支只做绑定；同时要防止 `opt.rs` 的语句重排
导致「块编译序 ≠ 收集序」（可用按名匹配降低风险）。

### 13.6.4 验证与门禁（回退后）

```text
# 遮蔽回归已消除（复现用例）
$env:ALUKA_CONF_FILTER="lang-more"; cargo test -p aluka-cli --all-features \
    --test conformance_node22_test -- --nocapture
Result: 24/24 passed, 0 invalid

$ cargo fmt --all --check                                    FMT=0
$ cargo clippy --all-targets --all-features -- -D warnings   CLIPPY=0
$ cargo test --workspace --all-features -j 4                 TEST=0（墙钟 221.5s）
聚合：suites=90 passed=632 failed=0 ignored=1
```

**工程隐患（复现一次）**：本轮构建时再次触发已登记的 rustc ICE
（`core::option::expect_failed`，增量缓存损坏）——按既有规避口径
`Remove-Item -Recurse target/debug/incremental` 后重试即过。

### 13.6.5 本轮编译器改动的最终范围

**入库（`cc0b922`）**：块内函数声明的**收集递归化 + 绑定预注册**（可见性修复）。
**未入库（已回退）**：`nested_preregistered` 槽复用方案（破坏遮蔽）。
**证据**：`p10b`/`p11` 两侧对拍（p11 全一致；p10b 在 aluka 侧抛 `TypeError`）；
`gen/gen-block-fn-decl-0002.cjs` 门禁内通过；`gen/deviations/gen-block-fn-decl-000{1,2}.cjs`
分别隔离「块前引用」与「同块 `let`/`const` 捕获」两项余差异。

### 13.4 门禁（修复后）

```text
# 1. 格式化门禁
$ cargo fmt --all --check                                    FMT_CHECK_EXIT=0

# 2. 严格 Clippy 门禁（零警告允许）
$ cargo clippy --all-targets --all-features -- -D warnings   CLIPPY_EXIT=0

# 3. 全工作区全量测试门禁（NODE=<v22.23.1>，CARGO_INCREMENTAL=0）
$ cargo test --workspace --all-features -j 4                 TEST_EXIT=0（墙钟 194.6s）
聚合：suites=90 passed=632 failed=0 ignored=1 ；SKIP=0

# 4. conformance 定向（新回归保护用例）
$env:ALUKA_CONF_FILTER="block-fn-decl"; cargo test -p aluka-cli --all-features \
    --test conformance_node22_test -- --nocapture
PASS gen/gen-block-fn-decl-0002.cjs
Result: 1/1 passed, 0 invalid
```

- 与本轮修复前的基线（§12.5：90 suites / 632 passed / 0 failed / 1 ignored）对照：
  **用例计数持平**——新增的 conformance 用例聚合在 `conformance_node22_test` 的单个
  `#[test]` 内（harness 逐例打印 PASS，故 passed 计数不变），属既有语料结构。
- 修复只动编译器（`aluka-compiler`），全工作区 632 例无一回归。

### 13.5 提交证据

```text
$ git commit -F -   # fix(compiler): 块内函数声明提升到函数作用域——修复「块内 function 不可见」引擎缺陷
[master cc0b922] 4 files changed, 290 insertions(+), 41 deletions(-)
 rewrite tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0001.cjs (93%)
 create mode 100644 tests/conformance/node22/cases/gen/gen-block-fn-decl-0002.cjs
```

只暂存目标文件（`.work/scratch/`、`.workbuddy/` 未入库）。改动文件：
`crates/aluka-compiler/src/module.rs`（修复本体）/
`tests/conformance/node22/cases/gen/gen-block-fn-decl-0002.cjs`（新增，门禁内回归保护）/
`tests/conformance/node22/cases/gen/deviations/gen-block-fn-decl-0001.cjs`（收窄为余差异）/
`.work/TODO/20260911/README.md`。

**⚠️ 明确未做（如实登记）**：§13.3 的「块执行前引用」精确对齐（绑定下移到块入口）
**本轮未实施**——需要引入「块内模板队列 + 块编译序 == 收集序」机制，改动面与回归风险
中等；当前实现方向为更宽松（提前可见）且不崩溃，故先以 deviations 用例隔离 + 文档登记，
建议独立一轮施行。**不得**在未实施的表述中声称已对齐。

---

## 14. 待办 30 · M5.1 收尾：`postMessageToThread` 真线程分支 + eval worker

> 触发指令：「继续M5」。本两项 = `.work/TODO/README.md` §M5.1 登记的最后两个缺口
>（「余 `postMessageToThread` 真线程分支、eval worker，20260910」）。

### 14.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | `postMessageToThread` 真实线程通路（按 Node 22.23.1 实测语义） | 返回 **Promise**（resolve `undefined`）；同线程投递 reject `ERR_WORKER_MESSAGING_SAME_THREAD`；目标线程/监听器缺失 reject `ERR_WORKER_MESSAGING_FAILED`；监听器抛错 reject `ERR_WORKER_MESSAGING_ERRORED`；投递目标为**目标线程 `process.on('workerMessage', (value, source))`**（与 parentPort 无关——Node 22.23.1 `lib/internal/worker/messaging.js` 实证）；支持主→worker、worker→主（destination 0）、worker→worker（经主线程中转）三向 | §14.3/§14.4 |
| 2 | `timeout` 参数面 | `postMessageToThread(tid, v, n)` 重载 = timeout；负数 reject `RangeError ERR_OUT_OF_RANGE`（文本 `The value of "timeout" is out of range. It must be >= 0. Received -1`）；超时未响应 reject `ERR_WORKER_MESSAGING_TIMEOUT`（`Sending a message to another thread timed out`） | §14.3/§14.4 |
| 3 | eval worker（`new Worker(src, { eval: true })`） | 真实线程路径现场编译源码执行；`require`/`parentPort`/`workerData` 可用；`__filename === '[worker eval]'`、`__dirname === '.'`；未捕获 TypeError → 主线程 `'error'` 收到 **Error 对象**（name/message 保真）+ `'exit'(1)`；语法错误 → SyntaxError + exit 1；非字符串 filename → 同步抛 `ERR_INVALID_ARG_TYPE`（eval:true 时为 `ERR_INVALID_ARG_VALUE`） | §14.3/§14.4 |
| 4 | 既有用例不回归 | `20-m5` / `21-m5` / `25-m5` / `26-m5` / `27-m5` 差分 + `builtins_phase6_proc_test` + 门禁三连全绿 | §14.6 |

**红线**：Node 22.23.1 为唯一 Oracle（本机 PATH node 已为 v22.23.1，直接对拍）；
错误文本逐字（含 Node 原文拼写「Cannot **sent** a message to the same thread」）；
做不到的如实登记为偏离，不放宽断言。

### 14.2 Oracle 取证（node v22.23.1，探针存 `.work/scratch/m51_finish/`）

- **postMessageToThread 通道真相**（`probe_a/b` + Node 源码 `lib/internal/worker/messaging.js`）：
  投递**不经 parentPort**——worker 启动时经 `createMainThreadPort` 向主线程注册
  `threadsPorts` 表；投递时在目标线程 `process.emit('workerMessage', value, source)`；
  无监听器 → `WORKER_MESSAGING_RESULT_NO_LISTENERS` → `ERR_WORKER_MESSAGING_FAILED`。
  首版探针（向 `parentPort.on('message')` 的 worker 投递）即因此得到 FAILED——
  修正实现方向：aluka 侧以 `process.on('workerMessage')` 为投递面。
- **返回值与错误面**（`probe_c`）：
  `typeof ret === 'object'`（Promise）；`postMessageToThread(0, v)`（主线程自投）→
  `Error|ERR_WORKER_MESSAGING_SAME_THREAD|Cannot sent a message to the same thread`；
  未知线程 → `ERR_WORKER_MESSAGING_FAILED|Cannot find the destination thread or listener`；
  `timeout=-1` → `RangeError|ERR_OUT_OF_RANGE|The value of "timeout" is out of range. It must be >= 0. Received -1`；
  worker 侧自投同线程 → 同 SAME_THREAD；worker→主（主无监听器）→ FAILED（「or listener」对主线程同样生效）。
- **官方错误文本**（Node v22.23.1 `lib/internal/errors.js`）：`ERR_WORKER_MESSAGING_ERRORED`
  = `The destination thread threw an error while processing the message`；
  `ERR_WORKER_MESSAGING_TIMEOUT` = `Sending a message to another thread timed out`。
- **eval worker**（`probe_e/f`）：eval 源码内 `require`/`parentPort`/`workerData` 全可用、
  正常结束 exit 0；未捕获 `TypeError('boom-eval')` → `'error'` 收到 **TypeError 对象**
  （`e.message === 'boom-eval'`）+ exit 1；语法错误 → SyntaxError + exit 1；
  `__filename === '[worker eval]'`、`__dirname === '.'`；
  `new Worker(42, {eval:true})` → 同步抛 `ERR_INVALID_ARG_VALUE|The property 'options.eval' must be false when 'filename' is not a string. Received true`；
  `new Worker(42)`（无 eval）→ 同步抛 `ERR_INVALID_ARG_TYPE|The "filename" argument must be of type string or an instance of URL. Received type number (42)`；
  `eval: 'yes'`（真值非 bool）不触发校验（按 eval 执行）。

### 14.3 实现记录

| 位置 | 内容 |
|---|---|
| `crates/aluka-vm/src/worker.rs` | 传输契约升级：新增 [`WorkerSource`]（File/Eval）；`WorkerInbound` 由裸 String 升级为信封枚举（PortMessage/WorkerMessage/RouteAck）；[`WorkerEvent`] 新增 RouteRequest/RouteAck，`Error(String)` 结构化为 `Error { name, message }`（主线程 `'error'` 收 Error 对象）；投递请求 id 进程级计数；结果码常量 0/1/2 对齐 Node |
| `crates/aluka-runtime/src/lib.rs` | spawn 钩子按 [`WorkerSource`] 分派：`run_worker_file`（原路径）+ `run_worker_eval`（`parse_source("[worker eval]")` 现场编译 → verify → 独立 Vm → 事件循环）；错误发送结构化（name/message；Thrown 异常提取 name/message）；eval worker `setup_cjs_eval(cwd)` |
| `crates/aluka-vm/src/modules.rs` | 新增 `setup_cjs_eval`：`__filename='[worker eval]'`、`__dirname='.'`、base_dir=cwd（相对 require 自 cwd 解析） |
| `crates/aluka-vm/src/builtins/worker_threads.rs` | ① `wt_worker_ctor`：filename 类型校验（非串+eval→ERR_INVALID_ARG_VALUE「Received true」；非串无 eval→ERR_INVALID_ARG_TYPE，`Received` 文本经 `received_inspect` 复刻）；eval+钩子→Eval 源码 spawn，伪 worker 路径维持失败登记；② `wt_post_to_thread` 重写：参数重载（transferList 数字→timeout）、timeout 校验（非数值 TypeError/negative RangeError ERR_OUT_OF_RANGE）、同线程拒绝、Promise 挂起表（主线程 `(origin, request_id)` / worker 侧 request_id）、主线程直发桥、worker 侧一律 RouteRequest 经主线程中转/投递、伪 worker 路径同步投递；③ worker 事件循环处理信封（PortMessage/WorkerMessage 投递+ack/RouteAck 结算）；④ `pump_real_workers` 处理 RouteRequest（destination 0 直投 / 中转 / 缺目标回 ack）与 RouteAck（origin=0 结算 / 转发回源 worker）、Error{name,message} 对象派发；⑤ 超时定时器经 `schedule_raw`（假时钟兼容）挂原生回调（自有键携带 origin/request_id/main_side），到期摘表并拒绝 ERR_WORKER_MESSAGING_TIMEOUT；⑥ 挂起表 resolver/reject 纳入 GC 根 |
| `crates/aluka-vm/src/builtins/timers.rs` | `schedule_raw` 提为 `pub(crate)`（timeout 定时器复用同一调度通路） |
| `crates/aluka-vm/src/interpreter.rs` | bootstrap 补挂 `Error.prototype.constructor`（对齐 RegExp 原型同款做法；`new Error('x').constructor.name === 'Error'`） |

**新增差分用例**（`tests/conformance/node22/cases/`）：`37-m5-post-to-thread.cjs`（主↔worker
双向 + 同线程/无目标/ERRORED/negative timeout 错误面）、`38-m5-post-to-thread-relay.cjs`
（worker→worker 经主线程中转 + 目标忙 500ms vs timeout 50ms 的真实超时到期）、
`39-m5-eval-worker.cjs`（eval 基本面/workerData/__filename/__dirname/未捕获
TypeError 对象/语法错误名/两类构造同步校验）。输出确定性编排：worker 侧直打、
主线程仅 exit 汇总，规避跨线程 stdout 交错的不确定性。

**登记偏离**（不放宽断言，逐项有 Node 依据）：
1. `Error` 族子类实例 `constructor.name` 恒为 `'Error'`（全引擎共用
   Error.prototype 单例，无 per-kind 原型树）——Node 为 `'TypeError'` 等；
   既有引擎形态，跨 M5 范围，用例规避该键（`e.name` 逐字对拍）。
2. 语法错误的 `'error'` 消息文本为 aluka 解析器自有（V8 文本体系不同）——
   用例只对拍 `e.name === 'SyntaxError'` 与退出码。
3. ack 回程以 mpsc RouteAck 近似 Node 的 SharedArrayBuffer + Atomics 应答
   （结果码语义一致：0/1/2）。
4. `filename` 的 URL 实例形态未接受（仅字符串）；`ERR_INVALID_ARG_TYPE` 的
   `Received` 对复杂对象按 format_value 回退（number/boolean/undefined/null/
   string 四类已逐字对拍）。
5. `timeout` 到期后迟到的 ack 静默丢弃（Node 迟到通知仅触 SAB，等价无副作用）。

### 14.4 差分验证

M5 差分门禁 `ALUKA_CONF_FILTER=m5` → **8/8 PASS, 0 invalid**（原 5 例 + 新增
37/38/39 三例，全部与 node v22.23.1 逐字节一致）：

```text
$env:ALUKA_CONF_FILTER="m5"; cargo test -p aluka-cli --all-features     --test conformance_node22_test -- --nocapture
PASS 20-m5-worker-threads.cjs
PASS 21-m5-cluster-http.cjs
PASS 25-m5-structured-clone.cjs
PASS 26-m5-fetch-bodyless.cjs
PASS 27-m5-worker-timer.cjs
PASS 37-m5-post-to-thread.cjs
PASS 38-m5-post-to-thread-relay.cjs
PASS 39-m5-eval-worker.cjs
----------------------------------------
Result: 8/8 passed, 0 invalid
```

三例覆盖面（判定口径 = stdout 与 node v22.23.1 逐字节一致）：
- **37**：主→worker 投递（worker 打印 `value + source`）；worker→主投递
  （resolve undefined）；监听器抛错 → `ERR_WORKER_MESSAGING_ERRORED`（主线程
  log 数组捕获到 `boom` 投递 + worker 侧 ack 文本双侧一致）；同线程（worker 自投）
  → `ERR_WORKER_MESSAGING_SAME_THREAD`（Node 原文拼写「Cannot sent …」）；无目标
  → `ERR_WORKER_MESSAGING_FAILED`；`timeout=-1` → `RangeError ERR_OUT_OF_RANGE`
  逐字（含 `Received -1`）；主线程 log 顺序与 exit 码。
- **38**：worker→worker 经主线程中转（`B got: {"kind":"hello-b"}` + ack resolve）；
  真实超时到期（目标忙 500ms，timeout 50ms）→ `ERR_WORKER_MESSAGING_TIMEOUT`
  逐字；terminate 收尾 exit 1。
- **39**：eval worker 现场编译执行、`workerData.n*2`、`__filename === '[worker
  eval]'`、`__dirname === '.'`；未捕获 `TypeError('boom-eval')` → `'error'` 收
  Error 对象（`instanceof Error === true` + name/message）+ exit 1；语法错误 →
  `SyntaxError` + exit 1；`new Worker(42, {eval:true})` 同步抛
  `ERR_INVALID_ARG_VALUE`（含 `Received true`）；`new Worker(42)` 同步抛
  `ERR_INVALID_ARG_TYPE`（含 `Received type number (42)`）。

**锚点用例同步复核**（红线要求）：`builtins_phase6_proc_test.rs` 的
`worker_missing_file_emits_error_and_exit_matches_go`——'error' 载荷由字符串改为
Error 对象，断言同步更新为 `werr fired: object true`。Node 依据：缺文件异步路径
（`./` 前缀）Node 实测同为 Error 对象（code `MODULE_NOT_FOUND`）+ exit 1，旧字符串
形态才是历史偏离；错误文本仍为 Go loader 文案（该用例为显式本地锚点）。**新增偏离
登记**：Node 对裸相对名（无 `./` 前缀）**同步抛** `ERR_WORKER_PATH`（v22.23.1 实测
`The worker script or module filename must be an absolute path or a relative path
starting with './' or '../'`），aluka 按 Go 口径接受裸相对名——既有本地锚点行为，
未在本轮改收窄。

### 14.5 门禁

```text
$ cargo fmt --all --check
（无输出，通过）

$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [...]
（0 error；各 crate 的「generated 1 warning」均为增量缓存 hard-link 环境提示，
 非代码警告，全仓既有）

$ cargo test --workspace --all-features
passed: 632, failed: 0（与 §13.4 基线 632 持平；新增 conformance 用例聚合在
 conformance_node22_test 单个 #[test] 内）
```

**首轮全量曾 1 失败**：`worker_missing_file_emits_error_and_exit_matches_go`
（'error' 载荷字符串 → Error 对象的形态变更）——按红线复核：Node 缺文件异步
路径实测同为 Error 对象 + exit 1（§14.4 锚点同步复核），断言更新后复跑全量
632/0。

### 14.6 提交证据

```text
$ git commit -F -   # fix(worker): M5.1 收口——postMessageToThread 真线程通路与 eval worker
[master c177b4b] 11 files changed, 1117 insertions(+), 74 deletions(-)
 create mode 100644 tests/conformance/node22/cases/37-m5-post-to-thread.cjs
 create mode 100644 tests/conformance/node22/cases/38-m5-post-to-thread-relay.cjs
 create mode 100644 tests/conformance/node22/cases/39-m5-eval-worker.cjs
```

只暂存目标文件（`.work/scratch/` 未入库）。改动文件：
`crates/aluka-vm/src/worker.rs` / `crates/aluka-vm/src/builtins/worker_threads.rs` /
`crates/aluka-vm/src/builtins/timers.rs` / `crates/aluka-vm/src/interpreter.rs` /
`crates/aluka-vm/src/modules.rs` / `crates/aluka-runtime/src/lib.rs`（实现本体）/
`crates/aluka-cli/tests/builtins_phase6_proc_test.rs`（锚点复核）/
`tests/conformance/node22/cases/37`/`38`/`39`（差分用例）/
`.work/TODO/20260911/README.md`。总表同步（`.work/TODO/README.md` M5.1 结项）随
后续 docs 提交入库。


---

## 15. 待办 31 · M5.2 `{"t":"e"}` ack 回程（worker 自发起断连挂起 → primary 回 ack → 收尾断连）

> 触发指令：「继续」。承接 §11 剩余缺口：`{"t":"e"}` 的 primary→worker ack 回程
> 未实现（worker 上报后即本地断连）。

### 15.1 开工前登记（目标 + 验收标准）

| # | 目标 | 验收标准 | 证据 |
|---|---|---|---|
| 1 | primary 侧收到 `{"t":"e"}` 后回 ack | `exitedAfterDisconnect=true` 置位（既有）+ 以同帧 `{"t":"e"}` 回程（Node `{ack: message.seq}` 的无 seq 近似，同通道单在途请求无歧义） | §15.2/§15.3 |
| 2 | worker 侧上报后**挂起**，收 ack 才收尾断连 | `cluster.worker.disconnect()` 同步返回后 `process.connected` **保持 true**（Node 实测口径：修复前为 false 即偏离点）；收到 ack 才 `process.disconnect()`（`'disconnect'` 事件晚于同步段）；上报失败（通道已断）立即收尾；挂起中重复调用 no-op；对端 EOF 时挂起失效（通道关闭路径自派发 `'disconnect'`） | §15.2/§15.4 |
| 3 | 既有用例不回归 | `m52_disconnect_test` / `m52_worker_msg_test`(6) / `m52_cluster_events_test`(4) / `m52_settings_test`(7) / `m52_http_cluster_test`(3) / `builtins_phase6_proc_test`(11) + M5 差分门禁 + 门禁三连全绿 | §15.4 |

### 15.2 Oracle 取证（node v22.23.1，探针 `.work/scratch/m52_ack/probe_ack.cjs`）

Node 侧 worker 自发起断连的同步段/异步段时序（实测连跑 2 次稳定）：

```text
worker:connected-before:true|worker:ead-before:undefined|worker:ret-self:true|
worker:state:disconnecting|worker:ead-sync:true|worker:connected-sync:true|   ← 关键：ack 未到通道不关
worker:proc-disc:false
primary:online|primary:disconnect|primary:exit:0:ead:true
```

aluka 修复前唯一差异点：`worker:connected-sync:false`（旧实现上报后**立即**
`process.disconnect()`）。Node 依据：`internal/cluster/child.js` 的
`_disconnect(false)` = `send({act:'exitedAfterDisconnect'}, () => process.disconnect())`
——send 回调在 primary ack 后才触发。

### 15.3 实现记录

| 位置 | 内容 |
|---|---|
| `crates/aluka-vm/src/builtins/cluster.rs` | ① primary 侧 `dispatch_worker_frame` 的 `"e"` 分支：置 `ead=true` 后 `send_to_worker(worker_id, {"t":"e"})` 回 ack；② worker 侧 `worker_self_disconnect_impl`：自发起时上报成功 → 置 `WORKER_ACK_PENDING` 挂起并**提前返回**（不 `process.disconnect`）；上报失败（通道已断）→ 立即收尾；挂起中重复调用 no-op；③ worker 侧 `dispatch_self_frame` 新增 `"e"` 分支：消费挂起标记（`replace(false)`）→ `process_disconnect` 收尾。`WORKER_ACK_PENDING` 为线程局部 `Cell<bool>`，对端 EOF 时挂起自然失效（通道关闭路径派发 `'disconnect'`） |
| 文档 | `cluster.rs` 模块头偏离登记改写为已实现；`cluster_ipc.rs` 帧协议文档改 `{"t":"e"}` 为双向语义；`FRAME_EXITED_AFTER_DISCONNECT` 常量注释同步 |

### 15.4 验证与门禁

**探针复跑**（修复后 aluka 与 node 逐字一致，输出见 §15.2）：差异点
`worker:connected-sync` 由 `false` → `true`，其余 7 行不变。

**新增 e2e**：`m52_disconnect_test.rs` 第 3 例
`cluster_worker_disconnect_ack_roundtrip_matches_node`（worker 侧同步段
`connected-sync=true` + 收 ack 后 `proc-disc connected=false`；primary 侧
`ead=true` + `exit code=0`，两侧与 Node 逐字节对拍）。

**提交**：`5af4d57`（fix(cluster) 4 files, +186/−25；总表 M5.2 行同步随下一
docs 提交入库）。

**既有 m52 回归**：`m52_disconnect_test`(2→**3**) / `m52_worker_msg_test`(6) /
`m52_cluster_events_test`(4) / `m52_settings_test`(7) / `m52_http_cluster_test`(3) /
`builtins_phase6_proc_test`(11) 全绿。

**门禁三连**：

```text
$ cargo fmt --all --check                → 通过（无输出）
$ cargo clippy --all-targets --all-features -- -D warnings
    → 0 error
$ cargo test --workspace --all-features
    → passed: 633, failed: 0（632 基线 + 新增 ack e2e 1 例）
```