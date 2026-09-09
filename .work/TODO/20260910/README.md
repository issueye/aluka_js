# 2026-09-10 · 每日 TODO（npm 功能复刻 + M6/M7 推进）

> 总 TODO 见 [../README.md](../README.md)；上一日：[20260909](../20260909/README.md)

**当前里程碑**：npm 复刻（M7.3 前置基建）+ M6/M7 推进　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+) 与真实 npm

工作分支：`feat/npm-m6m7`（worktree `../aluka_wt_npm_m6m7`，自 `281647e` 分叉）。

---

## 0. 开工前遗留处理（master）

上一轮 round7（M5.1 结构化克隆）代码完整但未提交。今日复核：

1. 修复 `worker_clone.rs` 测试代码 2 处 E0308（`construct_date` 返回 `Value` 而非 `ObjectRef`）；
2. 门禁全绿：fmt OK、clippy `-D warnings` 零告警、`cargo test --workspace
   --all-features` → **561 passed / 0 failed**（含新用例 25-m5-structured-clone 对拍）；
3. 提交 `281647e` `feat(m5.1): worker 结构化克隆——类型面/循环引用/transfer/detach + 纯消息 worker 保活`。

排障记录：conformance 单独跑（不带 `--all-features`）时 20-m5 失败——
`aluvm` 的真实 worker 线程钩子挂 `runtime` feature，缺 feature 时走伪 worker
降级路径报 `Cannot find module *.bc`。**门禁必须带 `--all-features`**（AGENTS
门禁命令本就要求，属调用口径问题非代码缺陷）。

---

## 1. 今日目标（可判定完成态）

1. **npm 功能复刻**：新 crate `aluka-npm`（纯 Rust，零 C 依赖）——
   `install/uninstall/run/ls/init/view` 子命令；npm semver 范围引擎（黄金
   用例对拍真实 `semver` 包）；registry packument 拉取 + SRI sha512 完整性
   校验；tar.gz 安全解包（strip 首层 + 路径逃逸防护）；npm 扁平化
   node_modules 布局（冲突嵌套）；Windows `.cmd` + sh 双 bin shim；
   package-lock v3 读写；生命周期脚本（preinstall/install/postinstall）。
   **验收**：`aluka-npm install express` 后 VM 跑通 express HTTP 服务（对拍 node）。
2. **M7.1 单二进制**：`aluka` 增加 `build` 子命令与 `npm` 子命令分发
   （流水线不变：源码 → 编译/校验 → VM）。
3. **M6.1 分代 GC 收口**：写屏障变异点审计补全 → 生产启用 minor；卡表化
   记忆集；基于存活率的动态堆伸缩阈值。
4. **M7.2 语料扩容**（持续推进项）：conformance 用例向 ≥1000 扩容。
5. **M6.2 / M6.3 / M7.3**：评估并按序推进（Time-permitting，证据不满足即不勾）。

---

## 2. 待办清单

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | round7 遗留验证与提交（master `281647e`） | `[x]` | M5.1 |
| 2 | aluka-npm：semver 范围引擎 + 黄金对拍 | `[x]` | npm 复刻 |
| 3 | aluka-npm：registry/tarball/完整性/解包 | `[x]` | npm 复刻 |
| 4 | aluka-npm：依赖树解析 + 扁平布局 + bin shim + lockfile | `[x]` | npm 复刻 |
| 5 | aluka-npm：install/run/ls/init/view 子命令 + e2e（express 对拍） | `[x]` | npm 复刻 |
| 6 | M7.1：`aluka run` 自动构建 + `aluka build` + `aluka npm` 分发 | `[x]` | M7.1 |
| 7 | M6.1：写屏障审计 + minor 启用 + 卡表 + 动态堆 | `[ ]` | M6.1 |
| 8 | M7.2：conformance 扩容（滚动） | `[ ]` | M7.2 |
| 9 | 门禁验证（fmt / clippy -D warnings / cargo test 全绿） | `[ ]` | 门禁 |
| 10 | 真实证据回填与 diff 复审 | `[ ]` | 证据闭环 |

---

## 3. 达成目标证据（真实证据闭环）

### 待办 2 · aluka-npm semver 引擎

**结论**：达成——与真实 `semver` npm 包（node-semver 7.7.3，Node 22 生态
权威实现）黄金对拍 27 版本 × 79 范围 = **2133 对全量一致**（覆盖精确/等号/
v 前缀/x-range/波浪/插入/原语/连字符/并集/AND 复合/预发布守卫/空白容忍）。

**证据类型**：命令证据 + 产物证据

```bash
$ cargo test -p aluka-npm --test semver_golden_test
test result: ok. 3 passed; 0 failed
# 语料: crates/aluka-npm/tests/semver_golden.json（真实包一次性生成固化）
```

修复记录：对拍暴露 2 处引擎偏差并修正——连字符完整上界应为 `<=` 含端点
（`1.2.3 - 2.3.4`）；算子与版本间留白形态（`>= 1.2.3`）需预合并。

### 待办 3/4 · registry/tarball/依赖树/布局

**结论**：达成——真实 registry 安装 express@4.22.2 全树 71 新装 + 57 hoist
复用；SRI sha512 完整性校验；zip-slip 防护测试（手工构造恶意 tar 条目）；
幂等重装（闭包下钻补全残缺树）；Windows `.cmd` + sh 双 bin shim；
package-lock v3 固化。

### 待办 5 · express e2e

**结论**：达成——devplan 交付物 `npm i express && aluka app.js` 全链路：
`aluka npm install express@^4.19.2` → `aluka run server.js`（自动构建 139
模块字节码镜像）→ VM 跑通 express HTTP 服务，`GET /`（JSON）与
`GET /echo/:word` 响应与 Node.js 22.23.1 **逐字一致**。

**证据类型**：命令证据（`cargo test -p aluka-cli --features runtime --test
npm_install_e2e_test` 全流程固化，registry 不可达时安全跳过）

```bash
$ cargo test --workspace --all-features
TEST-EXIT=0, sum passed: 575, 0 failed
```

### 顺带修复 · Number 静态方法全线失效（npm e2e 暴露）

`Number.isInteger/isSafeInteger/isFinite/isNaN/parseInt/parseFloat` 普通调用
恒返回 undefined——`number_static` 从 receiver（Number 构造器对象）推导方法
名，应从被调函数（pending_native_name）推导。is-odd 生态包实测暴露，修复后
is-odd/is-number 依赖链 VM 跑通。

---

## 4. 自动化门禁结果（全绿才可交付）

```bash
$ cargo fmt --all --check          # 通过
$ cargo clippy --workspace --all-targets --all-features -- -D warnings  # 零告警（CLIPPY-DONE）
$ cargo test --workspace --all-features
TEST-EXIT=0
sum passed: 575   # 77+ 测试目标，含 npm_install_e2e / semver_golden / conformance
```

---

## 5. 复审结论与偏差记录

- **git diff 复审**：变更与今日目标一致，无无关夹带；
- **偏差记录**：① registry 历史脏数据（`"engines": ">=0.10.40"` 字符串形态）
  需宽容反序列化——npm 生态元数据非规范形态客观存在；② `aluka run` 自动
  构建的项目边界以最近 package.json 定界（避免祖先链 node_modules 误触）；
  ③ build 横幅改走 stderr（stdout 只留程序数据，对齐 CLI 惯例）。
