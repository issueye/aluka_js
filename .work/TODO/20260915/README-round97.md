# 2026-09-15 · 续轮 TODO（M7.2 轮九十七：`process.argv[0]` 语义修正）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round96.md](./README-round96.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS（本机 v22.3.0）

**本轮范围**：轮九十五/九十六 §6 登记的「`process.argv` 未透传命令行参数」。

**结论**：**登记描述不准确，实测更正**——参数**一直是透传的**（A/B/C/D 四条路径
`process.argv.slice(1)` 均正确）。真正的缺陷是 **`argv[0]` 在源码构建路径下指向内部
构建产物 `.bc`**，与仓库自述约定（`argv[0]=脚本路径`）及 Node 语义不符。本轮修正该语义
并补回归测试。

---

## 1. 待办与结果

| # | 待办 | 结果 |
|---|---|:---:|
| 1 | 复测「`process.argv` 未透传」是否成立 | `[x]` 实测**不成立**（参数透传正常），更正登记 |
| 2 | 定位并修正 `argv[0]` 语义（构建路径暴露 `.bc` 产物） | `[x]` §2 |
| 3 | 覆盖四条调用路径的语义一致性 | `[x]` §3.1 |
| 4 | 补回归测试（两条路径 + 参数透传） | `[x]` §3.2 |
| 5 | 门禁（fmt / clippy / 全量 test） | `[x]` §4 |

---

## 2. 缺陷根因与修复

### 2.1 实测更正：参数透传本就正常

```text
# 探针：console.log('argv=' + JSON.stringify(process.argv))
A) aluka run argv_probe.js one two      → argv=["aluka_build\\argv_probe.bc","one","two"]
B) aluka run aluka_build\\argv_probe.bc one two → argv=["aluka_build\\argv_probe.bc","one","two"]
C) aluvm run aluka_build\\argv_probe.bc one two → argv=["aluka_build\\argv_probe.bc","one","two"]
D) aluka run p.js one two（无 node_modules 直执行）→ argv=["p.js","one","two"]
```

四条路径 `slice(1)` 均为 `["one","two"]` ⇒ **参数透传无缺陷**。轮九十五/九十六的
「`process.argv` 未透传」系早期探针（`probe-bisect2.js`）的**观测偏差**（当时
`process.argv` 打印为 `["aluka_build\\...bc"]` 是因为探针脚本以**环境变量**而非命令行
参数传参，且构建路径下 `argv[0]` 恰好是 `.bc` 路径，两个现象叠加被误读为「参数丢失」）。

### 2.2 真实缺陷：`argv[0]` 暴露内部构建产物

`aluka run <源码>` 在项目有 `node_modules` 时先构建镜像再执行，而执行装配把**字节码路径**
当作 `argv[0]`（`bc_entry::inject_process_argv(input, …)`），于是脚本看到
`process.argv[0] = "aluka_build/app.bc"`——**Node 语义下 `argv[1]`（本运行时无 exe 槽位
则为 `argv[0]`）应是用户运行的脚本**。影响：脚本内 `process.argv[0]`/`__filename` 类探测、
按 `argv[0]` 判分支的 CLI 程序、以及 `cluster` 的 `exec` 推导（`cluster.rs::current_script`
以 `vm.entry_file` 为准，构建路径下会得到 `.bc` 路径）。

**修复**（保持既有「无 exe 槽位 = `[script, ...cli]`」约定，仅修正首元素来源）：

- `aluka-runtime/src/bc_entry.rs`：`execute_bc` 抽出 `execute_bc_with_script(input, script, args)`，
  `argv[0]` 取 `script.unwrap_or(input)`；`execute_bc` 等价于 `script=None`（字节码直执行语义不变）；
- `aluka-runtime/src/lib.rs`：`Runtime::execute_bc_file` 同样抽出
  `execute_bc_file_with_script(path, script, args)`；导出 `execute_bc_with_script`；
- `aluka-cli/src/main.rs`：`run_command` 构建路径传 `Some(script)`；
  `test_command` 的构建路径传 `Some(file)`（用例源文件同理不应暴露 `.bc`）。

字节码**直执行**（`aluka run app.bc` / `aluvm run app.bc`）语义**保持不变**——此时
`argv[0]` 就是用户所指的字节码路径。


---

## 3. 达成证据

### 3.1 四条路径的 `argv[0]` 语义（修复后实测）

```text
A) aluka run argv_probe.js one two          → argv0=argv_probe.js          endsBc=false  rest=["one","two"]
B) aluka run aluka_build/argv_probe.bc …    → argv0=aluka_build/argv…bc   endsBc=true   rest=["one","two"]
C) aluvm run aluka_build/argv_probe.bc …    → argv0=aluka_build/argv…bc   endsBc=true   rest=["one","two"]
D) aluka run p.js（无 node_modules）        → argv0=p.js                  endsBc=false  rest=["one","two"]
```

- **A/D（源码路径）**：`argv[0]` = 用户运行的源脚本（修复前 A 为 `aluka_build/argv_probe.bc`）；
- **B/C（字节码直执行）**：语义不变，`argv[0]` 即用户所指的 `.bc`；
- 四条路径 `slice(1)` 参数**完整透传**。

### 3.2 新增回归测试

`crates/aluka-cli/tests/aluka_run_test.rs::test_aluka_run_argv0_is_user_script`
（两条路径各断言一次，锁定语义防回归）：

```text
# 无 node_modules（直执行路径）
argv0-basename:direct.js
# 有 node_modules（构建镜像路径）
endsBc:false
base:app.js
args:["p1","p2"]
```

```text
$ cargo test -p aluka-cli --all-features --test aluka_run_test
test test_aluka_run_argv0_is_user_script ... ok
test test_aluka_run_argv_forwarding ... ok
test result: ok. 6 passed; 0 failed
```

### 3.3 既有 argv/cluster 依赖套件（无回归）

```text
$ cargo test -p aluka-cli --all-features --test m52_settings_test   → 7 passed; 0 failed
$ cargo test -p aluka-cli --all-features --test aluvm_test          → 4 passed; 0 failed
```

---

## 4. 门禁（全绿）

```text
$ cargo fmt --all --check                                    FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings    CLIPPY_EXIT=0
$ cargo test --workspace --all-features --no-fail-fast -- \
    --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
（见 §4.1）
```

### 4.1 全量结果（实测）

```text
TEST_EXIT=0
suites=92   passed=653   failed=0        ← 653 = 轮九十六的 652 + 本轮新增回归测试 1
test node22_conformance_matches_node_stdout ... ok
test test262_subset_conformance ... ok
```

---

## 5. `git diff` 复审

```text
 crates/aluka-runtime/src/bc_entry.rs       | execute_bc → execute_bc_with_script（argv[0] 可显式指定）
 crates/aluka-runtime/src/lib.rs            | execute_bc_file → …_with_script；导出 execute_bc_with_script
 crates/aluka-cli/src/main.rs               | run_command / test_command 构建路径传源脚本
 crates/aluka-cli/tests/aluka_run_test.rs   | 新增 test_aluka_run_argv0_is_user_script
```

逐块审核：仅含本轮语义修正 + 回归测试；无夹带改动、无调试残留。

---

## 6. 仍未修复（登记）

| # | 项 | 说明 |
|---|---|---|
| 1 | 无 exe 槽位 | 本运行时 `process.argv = [script, ...cli]`（Node 为 `[exe, script, ...cli]`），属既有架构约定（`cluster.rs` 文档已声明并有等价换算）；本轮**未改**该约定，仅修正首元素来源 |
| 2 | `Error` 实例缺自有 `stack` 属性 | `Object.getOwnPropertyNames(err)` 少 `stack` |
| 3 | 根目录 `aluka.exe` 陈旧 / `--capabilities` 报 `native: 0` | 轮九十三既有登记 |
| 4 | `deepEqual` 宽松语义简化 | 复用 `deep_strict_equal`（跨类型叶子值未覆盖） |
| 5 | 事件循环真实时钟保真度 | 现为虚拟时钟推进 |
