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
