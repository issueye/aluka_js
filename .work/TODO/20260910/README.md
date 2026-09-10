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
| 7 | M6.1：卡表写屏障 + 根审计补漏 + 动态堆伸缩 + minor 生产启用 | `[x]`（基础闭环；极端压力残留项登记） | M6.1 |
| 8 | M7.2：conformance 扩容——生成器 + 验证分区 + 差分缺陷修复 | `[~]`（836 例进语料库 / 175 例偏差登记 / 1008 生成） | M7.2 |
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
### 待办 7 · M6.1 分代 GC 基础闭环

**结论**：达成（基础闭环）——卡表写屏障 / 根审计补漏 / 自适应堆伸缩 /
minor 生产启用全部落地；生产形态全量门禁 577 passed / 0 failed。

**交付明细**：
1. **卡表写屏障**：`GcState.cards`（每 64 槽位一字节）替换 `Vec<u32>` 记忆集
   （原 `contains` O(n) 去重）；minor 按脏卡扫描「老写新」容器，存活期跨回收
   的引用显式重卡；**晋升置卡守卫**（升代对象持年轻引用必须置卡——
   perf_hooks 实测暴露的经典 tenuring 漏洞）；
2. **变异点屏障补全**：Map.set/Set.add、Promise `.then` 处理器注册、
   `Object.defineProperty` 访问器注册（中央漏斗 set_property 原已覆盖
   Ordinary/Array/Closure/NativeCtor/NativeFn 全变体）；
3. **根审计补漏（19 处）**：Vm 字段 `require_fn`/`fs_object`/`env_object`/
   `objproto_has_own`/`last_entry_async_promise`/`require_bases` 键；
   静态表 provider ×18（NS_EMITTERS、EMITTER_STORE、NET_SHARED、
   DGRAM_SHARED、PORT_STATES/ENV_DATA、http LISTENERS/PENDING_EVENTS、
   http2、zlib DELIVERIES、async_hooks、domain、diagnostics、readline、
   sqlite TXNS、test ×3、vm/module 原型单例、**分派 TLS
   CURRENT_RECEIVER/PENDING_CALLEE**）；`StreamState.errored` 补入既有
   provider；**reflect::materialize 的 mem::take 窗口**钉扎补根；
4. **builtin 装配窗口挂起回收**（`gc_suspended` 计数，与 JIT 帧内跳过
   回收同一不变量；register_all 原以 `let _ =` 吞错改为暴露错误）；
5. **自适应堆伸缩**：minor 阈值按上轮存活率动态调整（地板 4096，
   存活率高抬升至多 16×/全死亡压回地板），major = 8×minor；
   替换原固定 2000 万次分配阈值（实际从不触发）；
6. **minor 生产启用**：push_object 分配漏斗内 minor/major 双代自然触发；
7. **压力验证模式**：`ALUKA_GC_STRESS=<N>`（每 N 次分配强制回收）+
   `ALUKA_GC_MODE=major|minor`（单路诊断）——审计从人工枚举转为机器验证。

**压力模式实战揪出并修复的缺陷（非压力下全部潜伏）**：模块装配期悬垂
（Cannot find module 间歇性）、require 入口函数被回收（`node:stream is not
a function`）、Reflect 窗口全模块失根、晋升误收（performance 属性缺失）、
分派 receiver 悬垂、流 errored 值丢失。

**证据类型**：命令证据

```bash
$ cargo test --workspace --all-features   # 生产形态（自适应阈值）
TEST-EXIT=0, sum passed: 577, 0 failed
$ cargo clippy --workspace --all-targets --all-features -- -D warnings  # 零告警
$ cargo test -p aluka-vm --lib gc        # 15 passed（含卡表粒度/晋升置卡/自适应阈值新用例）
```

**已知项收敛（本轮闭环）**：极端压力（stress 8–4096 全区间）复现并修复
三类根因——
1. **分配漏斗旁路**：`alloc_promise_resolver` 直接 `heap.push` 不登记
   ages/is_free 侧表也不触发 GC（违反 push_object 唯一入口不变量），
   major 清扫越界 panic——改走 push_object；
2. **挂起守卫失效**：`gc_suspended` 计数字段/方法虽在，但 push_object
   的 `&& gc_suspended == 0` 守卫因先前补丁脚本中途断言失败从未落地
   （编译无告警——字段在 suspend/resume 方法里被读到）——补上守卫；
3. **出生水位线**（`BIRTH_WATERMARK=4096`）：原生 handler 跨分配构建
   对象（createHash/createServer/Readable 等）期间无 VM 侧根、单周期
   宽限期不够（构建可跨两次回收）——出生后 N 次分配内不可回收，系统性
   覆盖该类窗口；压力模式下 crypto/流实例实测验证。

**压力验证终态**：`ALUKA_GC_STRESS=8/16/64/256/1024/4096` 全区间 +
全量套件 **578 passed / 0 failed**；生产形态门禁同绿；GC 单测 16 例
（新增出生水位线/宽限撤出/卡表粒度/晋升置卡/自适应阈值用例）。

### gcPressure 基准（M6.1 验收线：对标 V8 峰值内存 ≤3x）

**结论**：达成——**1.35x PASS**（aluka 183.3 MB vs node 136.2 MB 峰值
工作集，2000 万对象波浪负载；`cargo run -p aluka-cli --features runtime
--example gcpressure`，debug 构建、比值含 V8 逃逸优化偏保守因素）。

### 待办 8 · M7.2 conformance 语料扩容（生成器 + 验证分区）

**结论**：部分达成——**1008 例生成语料 × 41 域**，其中 **836 例差分验证通过
进入 conformance 语料库**（门禁每次全跑；手写 25 例 + 生成 836 例 = 861 例
实跑），**175 例分歧**自动化登记于 `cases/gen/DEVIATIONS.md`（含 node/vm
双侧输出，作为下轮修复工作面）。距 ≥1000 全绿目标尚差：偏差修复依赖下述
系统性缺口收敛。

**交付物**：
- `cases/gen/gen.mjs`：语料生成器（21 域表达式矩阵 + 5 批方法×参数组合
  矩阵，输出经 JSON 规范化、错误名对拍；确定性纪律：禁时间/随机/环境面）；
- `cases/gen/partition.py`：验证分区器（借 Rust runner 差分 + Python
  三进程明细采集；一致进 gen/、分歧进 deviations/ 并重写偏差清单）；
- conformance runner 支持 `cases/` 子目录递归与相对路径运行。

**本轮差分暴露并修复的引擎缺陷（10 类，全部真实语义面）**：
1. **算术强制转换通路分裂**：Sub/Div/Mod/Pow/Neg/UnaryPlus/位移走残缺的
   自由函数 `to_number`（字符串一律 NaN）而 Mul 走 `to_number_value`——
   统一为字符串感知路径（`"5" - "2"` 现为 3）；
2. **字符串数字解析**：空串应为 0（原 NaN）、`0x/0o/0b` 进制前缀；
3. **`>>>` 负数**：`as u32` 饱和转换把负数压成 0（`-16 >>> 28` 应为 15）；
4. **NaN/Infinity 全局缺失**（`typeof NaN` 原为 "undefined"）；
5. **`Object.is` 缺失**（规范 SameValue：NaN 同值真、+0/-0 异值）；
6. **数字→字符串规范格式**：新增 `js_number_to_string`（最短有效数字 +
   指数切换规则：`Number.EPSILON` → `2.22...e-16`、`String(-0)` → "0"、
   `1e21` → "1e+21"），替换 Rust `{}` 格式化（无指数形态）；
7. **词法科学计数法**（`1e21` 原被拆成 `1`+`e21` 直接解析失败）；
8. **`Array.from`/`Array.of` 缺失** + `reduceRight` 无初值语义（应从末元素起）
   + `toSorted` 忽略比较器；
9. **Promise JSON 序列化**（`JSON.stringify(Promise.resolve(1))` 应为 `{}`）；
10. **数组 ToNumber / 对象 ToPrimitive 拼接**（`Number([])`=0、`Number([7])`=7、
    `[] + []`="", `[] + {}`="[object Object]"）。

**未收敛的系统性缺口（175 例偏差的根因，下轮工作面）**：**原型方法属性
读面**——`typeof "abc".toUpperCase` 为 undefined（调用被按名拦截可用，
属性读不物化）；连带 Promise 静态方法属性（`typeof Promise.resolve`）、
`constructor.name`、`class` 表达式解析（`typeof class {}`）、
`Math.cbrt/log1p` 等末位 ULP 与 V8 对齐、`toString(radix)` 小数部分、
`toFixed(0)`、`padStart` 空串接收者、util.inspect/console.log 对象格式
等。DEVIATIONS.md 已逐条登记双侧输出，收敛路径清晰。

- **偏差记录**：① registry 历史脏数据（`"engines": ">=0.10.40"` 字符串形态）
  需宽容反序列化——npm 生态元数据非规范形态客观存在；② `aluka run` 自动
  构建的项目边界以最近 package.json 定界（避免祖先链 node_modules 误触）；
  ③ build 横幅改走 stderr（stdout 只留程序数据，对齐 CLI 惯例）。

---

## 待办 9 · M5 复审缺陷修复（评审后续轮）

> 评审结论见 [./README-m5-review.md](./README-m5-review.md)。本轮按报告 §6.2
> 「证据价值 ÷ 成本」顺序收敛 M5 范围内的问题，**不触碰**引擎级根因项。

### 开工前登记（目标 + 验收标准）

**已完成的前置动作**：`cargo test`（无 `--all-features`）会在
`crates/aluka-cli/src/bin/aluvm.rs:54` 因 `aluka_runtime` 未链接而硬编译失败；
评审期一次全量测试在仓库留下 0 字节垃圾文件 `crates/aluka-cli/process.exit(1))`，
**已删除**，工作树恢复干净（HEAD `8db31df`）。

| # | 任务 | 预期目标 | 验收标准 |
|---|---|---|---|
| 1 | **证据完整性**：修 `assert_e2e_matches_node` 静默跳过 | 区分「node 缺失 → 可见跳过」与「node 具备能力但探针失败/输出不符 → **失败**」；取消无 `else` 的静默通过 | ①本机（Node 无 `node:sqlite`）该用例以**可见 SKIP 标记**通过，不再是"无声绿"；②构造「node 存在且能力具备但输出不符」的对照，证明会真失败 |
| 2 | **fetch 响应定界补全**（M5.2 P0 残留） | 覆盖 bodyless 状态（1xx/204/304）与 HEAD 请求；读循环区分「IO 错误」与「响应结束」，禁止 `status=0` 伪成功；chunked 终止块不再裸扫描 | ①`/204` 响应 <1s（修复前实测 10013ms，Node 对照 5ms）；②对「只 accept 不发字节」的服务端不再返回 `status=0` 成功响应；③`21-m5`、`20-m5`、`25-m5` 与 phase9 保持绿 |
| 3 | **worker→主线程消息定时器饥饿**（M5.1 新发现） | 主线程存在待触发定时器时仍须轮询 worker 通道 | ①定时器 1000ms 内 `terminate` 不再丢消息；②定时器 2000ms 用例消息在定时器**之前**到达（当前被推迟到之后）；③无定时器路径不回归 |
| 4 | **M5 并发回归用例入库** | conformance 语料补 ≥2 连接并发 fetch、bodyless 响应、定时器+worker 混用的差分用例 | 新用例在本机 Node 上可跑（`node rc=0`）且与 aluka 逐字节一致；全量 conformance 的 `invalid` 计数不增加 |
| 5 | **门禁三连 + 证据回填** | 全部改动通过 `fmt` / `clippy -D warnings` / 全量测试 | 三连 exit 0，证据（原始命令输出）回填本节 |

### 本轮范围外（显式声明，避免静默丢弃）

- **引擎原型方法属性读面**（`constructor` / `instanceof` / `Symbol.toStringTag` /
  `Date.prototype.*`）——M5.1/M5.3「逐字一致」的公共前提，也是 `cases/gen`
  170 例偏差的主因，量大，另立专项；
- **`Map`/`Set` 键语义**（数字键与字符串键同键，实测 `new Set([3,'3']).size===1`）——
  评审建议提级为独立 P0，但属引擎核心数据类型，**不在 M5 范围内**，另立专项；
- M5.1 其余语义偏离（getter 求值 / SAB 拒绝 transfer / 未入图 buffer detach /
  Invalid Date / Error 实例）、M5.2 IPC 面与 `Connection: close` 服务端语义、
  M5.3 ctor options 与真预编译、M5.4 Timer Mock 与 CLI 运行器。

### 交付摘要

**5/5 项完成，门禁三连全绿。**

#### 1. 证据完整性（P0 伪证据修复）

- `crates/aluka-cli/tests/common/mod.rs`（+170/-28）：新增 `node_bin` /
  `node_available` / `node_version` / `node_supports_module` /
  `module_min_node_version` / `NodeOutcome` / `node_run_outcome`；
  `assert_e2e_matches_node_with_module(work, entry, required_module)` 做三态严格
  区分；`node_run` 与 `assert_e2e_matches_node` 保留为兼容包装（既有 3 个调用点
  与 `core_semantics_test.rs:500` 无需改动）。
- `crates/aluka-cli/tests/builtins_phase7_io_test.rs:222`：sqlite 对拍改走
  `Some("node:sqlite")` 能力门禁；5 条锚点断言原样保留。
- 本机实测原始输出（`cargo test -p aluka-cli --all-features --test
  builtins_phase7_io_test sqlite -- --nocapture`）：

  ```
  [SKIP node-e2e] probe.js: 未执行 Node 对拍（此处无对拍证据）——本机 node v22.3.0
  缺少用例所需模块 `node:sqlite`（需 Node ≥ 22.5.0）；本用例仅验证 aluka 自身输出
  test sqlite_node22_diff_e2e_matches_node ... ok
  ```

- **负向对照（证明"会真失败"，用替身 node 可执行文件）**：
  - 能力具备 + 输出不符 → `exit 101`，`assertion 'left == right' failed: e2e 输出与 Node.js 22 不一致（probe.js）`；
  - 能力具备 + node 退出码非 0 → `exit 101`，panic 附 stderr 原文（`STUB-NODE-STDERR-MARKER…`）；
  - node 缺失 → 可见 `[SKIP node-e2e]` + `ok`。

#### 2. fetch 响应定界补全（M5.2 P0 残留）

- `crates/aluka-vm/src/builtins/global/fetch.rs`：读循环三态化（`Ok(0)` 干净关闭 /
  读超时 / IO 错误），新增 `BodyDelim`（RFC 9112 §6.3 四条定界规则按优先级）、
  `parse_response_head`、`chunked_complete`（复用 `wire::take_chunked_with_body`
  作帧游走 + 严格终止块检查），删除裸 5 字节扫描与 `decode_chunked_body`，
  取消 `status` 的 `unwrap_or(0)` 兜底；新增 8 个 `delim_tests` 单测。
- `crates/aluka-vm/src/builtins/http/wire.rs`：`take_chunked_with_body` 放开为
  `pub(crate)` + `saturating_add` 溢出防护（唯一行为改动，对合法输入语义不变）。
- **验收实测**（同机、同一 aluka 服务端、HEAD 二进制对照）：

  | 场景 | 修复前 | 修复后 | Node 22 对照 |
  |---|---|---|---|
  | `/204` bodyless | 10013ms | **4ms** | 5ms |
  | 只 accept 不发字节 | `status=0 ok=false` 伪成功 | **抛 `TypeError`** | reject |

- 回归：`builtins_phase5_http_test` 10/10、`builtins_phase9_m4_test` 1/1、
  `builtins_phase7_io_test` 13/13。

#### 3. worker→主线程消息的定时器饥饿（M5.1 新发现）

- `crates/aluka-vm/src/microtask.rs`：`drain_macro_tasks` 中"整段 `sleep` 到
  到期时刻"改为 `wait_until_due`——**有活跃事件源时分片等待并在片间泵事件源**，
  无事件源时保留一次睡满（避免长定时器空转）。
- `crates/aluka-vm/src/builtins/worker_threads.rs:1168`：修 `if !pp_waiting`
  写反的 sleep 条件（挂监听时反而不睡 → 长驻应答型 worker 单核 100% 空转）。
- **red→green（同一工件、同机对照）**：

  ```
  RED  （microtask.rs 还原为 HEAD）：timer-fired / tree: alive / exit: 1   ≠ Node
  GREEN（修复后）                  ：tree: alive / exit: 1 / timer-fired   = Node（逐字节一致）
  ```

#### 4. M5 并发回归用例入库

- 新增 `tests/conformance/node22/cases/26-m5-fetch-bodyless.cjs`（跨进程 http
  server + 并发 fetch，锁 bodyless 与多连接场景）与
  `27-m5-worker-timer.cjs`（锁"worker 消息先于待触发定时器到达"）。
- Node 侧各跑 5 次输出哈希唯一（确定性）；入列后 `PASS 26-m5-fetch-bodyless.cjs`、
  `PASS 27-m5-worker-timer.cjs`，`invalid` 计数未增加。

#### 5. 门禁三连（回填真实输出）

```
cargo fmt --all --check                                          → exit 0（无 diff）
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                                 → exit 0，warnings=0 errors=0
cargo test --workspace --all-features                            → exit 0
                                                                 → 586 passed / 0 failed / 1 ignored
conformance 全量（--nocapture）                                   → Result: 864/864 passed, 3 invalid
```

- 586 较修复前 578 增加 8 例（= fetch 新增 `delim_tests` 8 例）；`invalid` 仍为
  原有 3 条（`03-require-esm` / `15-test-runner` / `16-m7-test-core`，均为本机
  Node 版本偏低所致，非 aluka 侧失败）。
- 语料 862 → 864 = 新增 2 条本轮用例。

#### 6. 本轮顺带发现、**未修**（另立专项，附不修的安全理由）

**A. `aluka npm init -y` / `npm install` 的"项目根"解析会越界到祖先目录**

- `crates/aluka-npm/src/commands.rs:297-306` `find_project` 从 cwd 逐级上溯**直到
  盘根**；`cmd_init`（:223）复用它，而 npm 语义是"作用于当前目录"。
- 本机 `C:\Users\User\package.json` 是**用户真实的 npm 工程**（11 个真实依赖，
  含 `is-odd@^3.0.1`、`semver`），于是：
  - 任意位于 `C:\Users\User\` 之下的目录执行 `aluka npm init -y` 都报
    `package.json 已存在`（实测：**空目录**亦报错，`EXIT=1`）；
  - `aluka npm install` 在同一情形下会把"项目根"解析到用户主目录 →
    **实际写入真实工程**（package.json / package-lock.json / node_modules）。

**B. 测试基建的 `node -e` 载荷被本机 shell/shim 破坏**

- `crates/aluka-cli/tests/npm_install_e2e_test.rs:17` 的载荷含 `=>process.exit(1))`；
  本机 `node` 经 nvmd shim 启动时 `>` 被当作重定向 → 载荷被截断（node 报
  `SyntaxError: Unexpected token ')'`），并在 CWD 生成 0 字节文件
  `process.exit(1))`。
- 后果：`registry_reachable()` 恒 false → 该 e2e **长期走 SKIP 分支假绿**；且每次
  全量测试都污染工作树（`crates/aluka-cli/process.exit(1))`，本轮已手动清理）。
- 修法（已验证有效、**因 A 的安全风险回退**）：载荷改用 `function(){…}` 形式，
  使其不含 `>`。正确顺序是**先修 A**（让 `npm init/install` 不越界），再加固探针。

**C. 回退记录**：本轮曾把 B 的修法落地，随后
`npm_install_and_vm_run_matches_node` 由"假绿 SKIP"变为真实执行并
**FAILED（`npm_install_e2e_test.rs:40` init 断言）**，根因即 A。鉴于继续执行会写入
用户主目录，**已回退该文件**（`git diff` 为空，测试恢复 SKIP 绿）。

#### 本轮遗留

- M5.1 其余语义偏离、M5.2 IPC 面与 `Connection: close` 服务端语义、M5.3 ctor
  options 与真预编译、M5.4 Timer Mock 与 CLI 运行器（均见 §范围外）；
- fetch 侧代码推定未实测项：同进程自请求互锁、流式响应无限阻塞
  （`fetch` 仍是同步阻塞实现，本轮的定界修复不改变这一点）；
- `find_project` 越界（§6-A）与探针加固（§6-B）需按序另立专项。

---

## 待办 10 · `find_project` 越界修复 + npm e2e 探针加固（待办 9 的闸门项）

> 承接 §待办 9 §6 的两个顺带发现。按"先修 A 再加固 B"的顺序，因为 B 的加固会让
> e2e 真正执行 `aluka npm install`——**若 A 未修，install 会把项目根解析到用户主
> 目录并写入真实工程**。

### 开工前登记（目标 + 验收标准）

**Oracle 实测（真实 npm 10.8.1，沙箱 `parent/package.json` + `parent/child/`）**：

| 命令 | 真实 npm 行为 | Aluka 现状 |
|---|---|---|
| `npm prefix` @ `parent/child` | `parent`（**上溯**） | `find_project` 上溯 ✅ 一致 |
| `npm init -y` @ `parent/child`（父目录有 package.json，cwd 无） | **写入 `parent/child/package.json`**（作用于 cwd，不上溯） | ❌ 解析到 `parent` → 误报「package.json 已存在」 |
| `npm init -y` @ cwd 已有 package.json | 覆盖重写（EXIT=0） | Aluka 报「已存在」（**有意保留**的安全折衷） |

| # | 任务 | 预期目标 | 验收标准 |
|---|---|---|---|
| 1 | **`cmd_init` 改用 cwd** | `init` 不再复用 `find_project`（npm 语义：作用于当前目录）；`install/uninstall/run/ls` 保持上溯（npm prefix 语义不变） | ①在 `C:\Users\User\` 之下的**空目录**执行 `aluka npm init -y` 成功生成 package.json（修复前报「已存在」、EXIT=1）；②祖先有 package.json 时 `init` 仍写入 cwd；③`install/ls` 的上溯行为不回归（`npm prefix` 语义保持） |
| 2 | **npm e2e 探针加固** | `registry_reachable()` 的 `node -e` 载荷去掉 `>`（箭头函数 → `function(){}`），使本机 shell/shim 不再把它当重定向 | ①探针真实返回可达（不再恒 false）；②`npm_install_and_vm_run_matches_node` 真实执行且通过；③全量测试后工作树**不再**出现 `crates/aluka-cli/process.exit(1))` |
| 3 | 门禁三连 + 证据回填 | — | 三连 exit 0，证据回填本节；含"加固探针前后 e2e 行为对照" |

### 交付摘要

**2/2 项完成，门禁三连全绿。**

#### 1. `cmd_init` 改用 cwd（npm 语义）

- `crates/aluka-npm/src/commands.rs:224`：删除 `cmd_init` 里的 `find_project(cwd)?`，
  改为 `installer::init(cwd)?`，并把 npm oracle 依据写进函数文档注释。
  **`find_project` 本身零改动**（`git diff` 仅两行：删调用、加文档），
  install/uninstall/run/ls 的上溯语义（`npm prefix`）保持不变。
- 验收实测：

  | 验收 | 命令 | 结果 |
  |---|---|---|
  | ① 空目录（位于 `C:\Users\User\` 之下，祖先有 package.json） | `aluka npm init -y` | **EXIT=0，`已生成 package.json`**；落盘位置仅 `child\package.json`（`"name": "child"`）。修复前：`package.json 已存在`、EXIT=1 |
  | ② cwd 自身已有 package.json | `aluka npm init -y` | `package.json 已存在`、EXIT=1（**有意保留**的与 npm 覆盖语义的偏离，见登记表） |
  | ③ 上溯语义不回归 | `aluka npm ls` @ `parent/child` | 解析到 `parent`（读到父级 `1.0.0`），与真 `npm prefix` 输出 `…\parent` 一致 |

#### 2. npm e2e 探针加固（`=>` 去除）

- `crates/aluka-cli/tests/npm_install_e2e_test.rs:15-22`：`-e` 载荷由箭头函数改为
  `function(){…}`，载荷不再含 `>`；并加注释说明成因，防止后人改回。
- **前后对照**（同一用例）：

  | | 修复前 | 修复后 |
  |---|---|---|
  | `registry_reachable()` | 恒 false（node 收截断载荷 → SyntaxError） | 真实返回可达 |
  | 用例行为 | 走 SKIP 分支，"ok"（0.12s，**假绿**） | **真实执行**：init → install → 校验 → VM 跑通，**ok（4.34s）**，无 SKIP 标记 |
  | 工作树污染 | 每次全量测试生成 `crates/aluka-cli/process.exit(1))` | **不再生成**（全量测试后 `git status` 无该文件） |

- 安全验证（本轮最关键）：探针真实化后 e2e 会真的 `aluka npm install`，故确认
  **未写入用户主目录工程**——`C:\Users\User\package.json`（mtime 2026-08-19 9:39:39）
  与 `package-lock.json`（2026-09-02 13:04:38）**mtime 未变**；
  `C:\Users\User\node_modules` 为 2022-03-21 创建的既有目录（553 条目，其中
  `is-odd` 日期 2026-08-19），与本次运行无关。测试临时目录由用例自清理，
  失败运行遗留的两个空目录已手动删除。

#### 3. 门禁三连（回填真实输出）

```
cargo fmt --all --check                                          → exit 0（无 diff）
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                                 → exit 0，warnings=0 errors=0
cargo test --workspace --all-features                            → exit 0
                                                                 → 586 passed / 0 failed / 1 ignored
conformance 全量（--nocapture）                                   → Result: 864/864 passed, 3 invalid
```

- 与 §待办 9 的基线一致（586/0/1、864/864/3）；本轮改动落在 `aluka-npm` 与测试基建，
  不影响 conformance 语料与差分通道。

#### 顺带观察（未处理，非本轮范围）

- `aluka npm ls` 在祖先解析正确的前提下，项目名打印为 `<unnamed>`（版本正确读出），
  而父级 `package.json` 明确有 `"name": "parent"` → `ls` 的名称显示面疑有独立缺陷，
  未深究，建议另立小专项复核。

---

## 待办 11 · Map/Set 键语义 Correctness（SameValueZero 化）

> 来源：M5 复审报告 §4.3 定级的**独立 P0**（超出 M5 范围，本轮单独立项）。
> round7 亦曾以「Set 键字符串化（3 与 '3' 同键）」登记为引擎既有面缺口。

### 开工前登记（目标 + 验收标准）

**缺陷（实测，改前基线）**：

| 表达式 | Node 22 | Aluka 现状 |
|---|---|---|
| `new Set([3, '3']).size` | 2 | **1** |
| `new Map().set(3,'n').set('3','s').get(3)` | `'n'`（size 2） | **`'s'`**（size 1） |

**根因**：`crates/aluka-vm/src/heap.rs:158` 的 Map/Set 表示把键字符串化——
```rust
/// 有序项集（键经 `to_property_key` 字符串化；Set 的 value = 元素原值）
entries: Vec<(String, Value)>,
```
`interpreter.rs:2797` 用 `to_property_key` 求键，比较处一律 `String` 相等
（`interpreter.rs:2889/2908/2912`）。于是数字/字符串/布尔/对象键被归一
（`3`≡`'3'`、`true`≡`'true'`、两个不同对象≡`"[object Object]"`）。
附带缺陷：`Map.forEach` 的键由 `alloc_string(k)` 重新分配（:2857），
不是原键对象；`heap.rs:655` 的 GC 扫描**只标记 value 不标记 key**（因键是 String）。

**目标**：键槽保留原始 `Value`，比较改用 **SameValueZero**
（`NaN`≡`NaN`、`+0`≡`-0`、对象按引用身份、字符串按内容），并保证插入序、
`forEach` 键身份、GC 根覆盖键槽。

**验收标准**：

| # | 用例 | 期望（Node 22） |
|---|---|---|
| 1 | `new Set([3,'3']).size` | 2 |
| 2 | `new Map().set(3,'n').set('3','s')` → `get(3)` / `size` | `'n'` / 2 |
| 3 | `new Set([true,'true',1,'1']).size` | 4 |
| 4 | `new Set([NaN, NaN]).size` | 1 |
| 5 | `new Map().set(-0,'z')` → `has(0)` / `size` | `true` / 1 |
| 6 | 两个不同 `{}` 键 / 同一对象键两次 | size 2 / 1 |
| 7 | 插入序：先 a 后 b，覆盖 a 后再 `keys()` | `[a, b]`（原序不变） |
| 8 | `map.forEach((v,k)=>{seen=k})` → `seen === 原键对象` | true（对象键同一性） |
| 9 | `[...set]` / `for...of` / `Array.from` / `entries/keys/values` | 与 Node 逐字一致 |
| 10 | `util.inspect(new Map([[1,'a']]))` | 键显示为数字非 `'1'` |
| 11 | worker 结构化克隆往返（数字键/对象键 Map、数字元素 Set） | 语义保持 |
| 12 | `ALUKA_GC_STRESS` 下对象键不被误回收 | 存活 |
| 13 | 门禁三连 | fmt/clippy/全量 + conformance 全绿、`invalid` 不增加 |

### 交付摘要

**核心目标达成：Map/Set 键语义与 Node 一致；并连带修复同源的 4 个既有缺陷。**

#### 1. 键语义 SameValueZero 化（任务本体）

- `heap.rs:158`：`entries: Vec<(String, Value)>` → **`Vec<(Value, Value)>`**（Set 保持
  key = value = 元素原值的双槽约定）；`alloc_map` 签名随动。
- `interpreter.rs`：键来源去 `to_property_key`（`args.first()` 直取原值）；
  快照类型随动；`forEach` 直传原键（键身份正确）；`Map.groupBy` 改 Vec + SameValueZero；
  `get/set/has/delete` 统一 SameValueZero；**补键写屏障**。
- `call.rs`：`new Map/Set(iterable)` 改走 `collect_iter_values`（见 §3）。
- `worker_clone.rs`：Map 键线格式由 `self.str()` 改为 `serialize_value`（读端
  `self.value()` 成对）、`T_SET` 键=元素原值、`push_map_entry` 改 `Value` 键 +
  **补写屏障**（原实现无屏障，属既有缺口）。
- **GC 关键点**：`heap.rs` 的 `trace_refs` Map 分支原**只标记 value**；键改 `Value`
  后补标记键槽（所有 GC 标记路径都经此处，一处生效）。

#### 2. 连带修复的既有缺陷（本任务外新发现，均已实测）

| # | 缺陷（改前实测） | 根因 | 修复 |
|---|---|---|---|
| A | **`["a","b"].includes("b")` → `false`**（连字面量都错） | `values_same_zero` 只做 `a == b`，而 `Value` 的 `PartialEq` 对堆字符串是**句柄比较** | 改为 `Vm::values_same_zero`：`ops::strict_eq`（已按内容比字符串）+ NaN 自等 |
| B | `[...[1,2].values()]`、`Array.from(map)`、`Object.fromEntries(map)` **静默为空** | `collect_iter_values` 按容器类型识别，迭代器对象落入空分支；`Array.from`/`Object.fromEntries` 各自按类数组处理 | 迭代器对象挂**真实 `next` / `Symbol.iterator` 属性**（`iter.rs::attach_iterator_surface` + `surface.rs` 两个 handler）；`collect_iter_values` 补迭代器排空分支；`Array.from`/`Object.fromEntries` 补可迭代优先分支 |
| C | `new Set("ab").size` → **0**、`new Map([[1,'a'],[1,'b']])` **不去重** | `call.rs` 的 Map/Set 构造只识别 `HeapObject::Array` | 改走 `collect_iter_values`（接受任意可迭代） |
| D | `JSON.stringify(new Map())` → **`null`** | `json_write` 对 Map 变体落入 `_ => "null"` | Map/Set 并入 Ordinary 分支 → `{}`（与 Node 一致） |

**其中 A 的波及面远超 Map/Set**（`Array.prototype.includes`/`indexOf` 是常用 API），
是本轮价值最高的发现。

#### 3. 验收结果（探针逐行对比 Node v22.3.0）

- **`accept_probe.js`（32 项：键语义 16 + 迭代器 16）**：改前 **46 行差异** → 改后
  **仅剩 1 行**，且为**有意登记的偏离**（见 §4）。
  关键项：`Set([3,'3']).size=2`、`Map` 数字/字符串键分离、`new Set([true,'true',1,'1']).size=4`、
  `NaN` 键、`±0` 同键、对象键身份、插入序、`delete` 后重加序、`forEach` 键身份、
  `[...it]`/`Array.from`/`Object.fromEntries`/`keys/values/entries` 全部一致。
- **`samezero_probe.js`（11 项数组/集合比较）**：**IDENTICAL**。
- **GC 键存活**：对象作 Map 键/Set 元素，20 万次分配逼 GC 后仍正确；默认、
  `ALUKA_GC_STRESS=16`、`=1024` 三种压力下输出一致。
- **worker 往返**：含数字/字符串/对象键的 Map 与 Set `postMessage` 往返后键类型与
  语义保持，**与 Node 逐字节一致**。
- **deviations 语料再判定**：`cases/gen/deviations/` 中 12 个 Map/Set 用例
  **10 个现已与 Node 一致**（改前全部分歧）；剩 2 个为**其他已登记缺口**
  （`gen-object-json-0025` 需 `structuredClone` 全局；`gen-eval-matrix-0008`
  需 `instanceof`，受原型属性读面所限）。
  > 按既有流程，重新分区应由 `cases/gen/partition.py` 执行，本轮**未手工搬移文件**。

#### 4. 本轮引入并登记的偏离（刻意折衷）

1. **迭代器 `next` / `Symbol.iterator` 挂为自有属性**：Node 挂在各迭代器**原型**上，
   `Object.prototype.hasOwnProperty.call(it,'next')` 在 Node 为 `false`、本实现为
   `true`（已写入 `iter.rs` 注释登记）。行为等价（可读、可调、自迭代），仅属性归属不同。
2. **Map/Set 上的用户自有属性不被 `JSON.stringify` 序列化**（`own_entries` 只读
   Ordinary 的 props）——已写入 `prims.rs` 注释。
3. `new Map(5)` 得空 Map（Node 抛 TypeError）——非可迭代实参未做类型校验，**未修**，
   本轮登记。

#### 5. 门禁（回填真实输出）

```
cargo fmt --all --check                        → exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                               → exit 0，warnings=0 errors=0
cargo test --workspace --all-features          → exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                                → Result: 864/864 passed, 3 invalid
```

586/0/1 与 864/864/3 **与本轮基线完全一致** —— 行为面大改但**零回归**。

#### 6. 本轮未做（另立专项，附理由）

- **迭代器内部标记属性泄漏**：`Object.keys([1,2].values())` → `["_isArrayIterator","_iterArray"]`
  （Node `[]`）、`JSON.stringify(iter)` → 内部结构（Node `{}`）。需把标记改为不可枚举
  或改用真实原型面，属表示层重构，**非本任务范围**；
- `util.inspect`/`console.log` 无 Map/Set 格式化特判（验收项 10 **未达成**，缺的是
  `util.inspect` 的 Map 显示实现，与键语义无关）；
- `structuredClone` 全局未实现；
- `instanceof Map` 为 false（原型属性读面，已登记的系统性缺口）。

#### 7. 验收项达成对照

| # | 验收项 | 结果 |
|---|---|---|
| 1-9, 11, 12 | 键语义 / 迭代 / worker 往返 / GC 存活 | ✅ 达成（探针逐行一致） |
| 10 | `util.inspect(Map)` 键类型显示 | ❌ **未达成**——`util.inspect` 无 Map 特判（既有功能缺口，非键语义） |
| 13 | 门禁三连 | ✅ 达成（零回归） |

---

## 待办 12 · 数组变异方法 `pop` / `shift` / `unshift` 修复

> 来源：本轮复核 `cases/gen/deviations/` 时发现——该目录里的 `gen-array-0037/0039/0040`
> 把「`[1,2,3].pop()` 返回 `undefined`」登记为**已知分歧**，但因为它位于隔离区
> （不参与门禁），864 例全绿也从未覆盖到它。这是"conformance 全绿 ≠ 引擎正确"的又一例证。

### 开工前登记（目标 + 验收标准）

**缺陷（实测，改前基线）**：

| 表达式 | Node 22 | Aluka 改前 |
|---|---|---|
| `const a=[1,2,3]; a.pop()` | `3`，`a` → `[1,2]` | **`undefined`，`a` 仍为 `[1,2,3]`** |
| `[1,2,3].shift()` | `1` | **`undefined`** |
| `[2,3].unshift(1)` | `2` | **`undefined`** |

即"返回值错 **且不改写数组**"——静默错误结果。同批探针确认 `push` / `splice` /
`sort` / `reverse` / `slice` / `map` / `join` / `at` 等**均正常**，问题被限定在这三个方法。

**根因**：解释器里存在**第二处**数组方法内联分派（`interpreter.rs` 的
`else if let Value::Object(r) = receiver` + `matches!(heap[idx], HeapObject::Array)`
分支，`push`/`map`/`sort`/`splice` 等在此），其中**缺 `pop`/`shift`/`unshift` 三个
分支** → 落到通用兜底 `Value::Undefined`；而 `surface.rs` 的注册表路径
（`array_method_dispatch`）只有 `pop` 分支、缺 `shift`/`unshift`。

**目标与验收**：

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | 补齐 `interpreter.rs` 数组内联分派的三个缺失分支 | `pop`/`shift`/`unshift` 返回值与数组改写与 Node 一致；空数组返回 `undefined`；`unshift` 多参顺序正确；`unshift` 补写屏障 |
| 2 | 补齐 `surface.rs` 注册表路径的 `shift`/`unshift`（`Array.prototype.pop.call(...)` 形态） | 两条路径行为一致 |
| 3 | 回归用例入门禁语料 | 新增 `28-array-mutators.cjs`；Node 侧确定性（多次运行同哈希）且与 Aluka 逐字节一致；`invalid` 不增加 |
| 4 | 门禁三连 | fmt / clippy / 全量 + conformance 全绿，零回归 |

### 交付摘要

**4/4 达成。**

- `interpreter.rs` 数组分派补 `pop`（删末元素并返回）、`shift`（删首元素、其余前移）、
  `unshift`（前插全部实参、返回新长度、**补写屏障**）三分支；
- `surface.rs` 的 `array_method_dispatch` 同步补 `shift`/`unshift`（`pop` 分支原本已正确）；
- 新增门禁语料 **`28-array-mutators.cjs`**（12 项：三大方法 × 空/非空、多参顺序、
  与 push/splice 混用、`length` 变化、字符串元素），把该缺陷从隔离区移入受保护范围。

**验收实测**：

```
arrmethods_probe.js（20 项数组方法）  →  与 Node IDENTICAL
28-array-mutators.cjs                 →  Node 侧 5/5 运行同哈希；Aluka IDENTICAL
gen-array-0037 / 0039 / 0040          →  MATCH（改前均为登记分歧）
cargo fmt --all --check               →  exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                      →  exit 0，warnings=0 errors=0
cargo test --workspace --all-features →  exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                       →  Result: 865/865 passed, 3 invalid
                                         （864 + 新增 1；invalid 未增加）
```

**本轮顺带登记（未修）**：`JSON.stringify.call(JSON, 3)` 在 Aluka 抛
`TypeError: [function Function] is not a function`（Node 返回 `3`）——
属原生函数上的 `Function.prototype.call`/`apply` 面缺口，与数组方法无关，另立。

---

## 待办 13 · `structuredClone` 全局接线 + `util.inspect` 的 Map/Set 格式化

> 承接 §待办 11 §6 的两条"小而立即可做"项：`structuredClone` 全局未接线、
> `util.inspect` 无 Map/Set 特判（后者是本人在 §待办 11 判定**未达成**的验收项 10）。

### 开工前登记（目标 + 验收标准）

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | `structuredClone(value[, { transfer }])` 全局接线 | 基本类型/对象/嵌套/循环引用/Map/Set/Date/RegExp 往返正确；与原值**互相独立**（改克隆不影响源）；`transfer` 移交后源 detach（`byteLength` 归零）且克隆保有字节；不可克隆值（函数/Symbol）抛 `DataCloneError`；无参抛 `TypeError` |
| 2 | `util.inspect` 的 Map/Set 格式化 | 与 Node 形态逐字一致：`Map(n) { k => v }`、`Set(n) { v }`、空集合 `Map(0) {}` / `Set(0) {}`、条目级字符串加单引号 |
| 3 | 两条回归用例入门禁语料 | Node 侧确定性（多次运行同哈希）且与 Aluka 逐字节一致；`invalid` 不增加 |
| 4 | 门禁三连 | fmt / clippy / 全量 + conformance 全绿，零回归 |

### 交付摘要

**4/4 达成。**

**1. `structuredClone`（复用既有序列化，未新写克隆逻辑）**

- `interpreter.rs`：全局名 `structuredClone` 注册 + `CALL` 分派分支（与
  `queueMicrotask` 同处，同属"全局原生函数"形态）；
- `worker_clone.rs`：新增 `Vm::structured_clone(args)`——取首参为值、第二参
  `{ transfer: [...] }` 为移交列表，复用 **`serialize` + `deserialize`**
  （与 worker `postMessage`、同线程 `json_roundtrip` 同一套自描述序列化），
  故类型面/循环与共享引用/transfer+detach/`DataCloneError` 语义与 worker 传值完全一致；
  无参时抛 TypeError（Node 文本 `The value argument must be specified`）。
- **复用价值**：未新增任何克隆算法，只接线——这是"小改动"判断成立的原因。

**2. `util.inspect` 的 Map/Set 格式化**

- `builtins/util.rs`：`inspect_value` 补 `HeapObject::Map` 分支（经 `is_set_instance`
  区分 Map/Set），条目级新增 `inspect_entry`（字符串加单引号）；
- 改前 `util.inspect(new Map([[1,'a']]))` → `[object Object]`，现为 `Map(1) { 1 => 'a' }`。

**验收实测**（Node v22.3.0 逐行对拍）：

```
sc2_probe.js（12 项 structuredClone）        →  核心语义全一致（见下"登记偏离"）
insp_probe.js（9 项 util.inspect）           →  IDENTICAL
29-structured-clone-global.cjs（16 项）      →  Node 5/5 同哈希；Aluka IDENTICAL
30-util-inspect-mapset.cjs（11 项）          →  Node 5/5 同哈希；Aluka IDENTICAL
cargo fmt --all --check                      →  exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                             →  exit 0，warnings=0 errors=0
cargo test --workspace --all-features        →  exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                              →  Result: 867/867 passed, 3 invalid
                                                （865 + 新增 2；invalid 未增加）
```

**本轮登记的两处刻意回避（用例层面已注明，避免伪对拍）**：

1. `structuredClone(new Map()) instanceof Map` —— Node `true` / Aluka `false`，
   受**原型属性读面**所限（§待办 11 亦命中此项）。用例改判 `size`/`get`/`has`，
   **不以 `instanceof` 作为通过条件**；
2. `DataCloneError` 的 **message 文案**——Node 含被克隆值的源码文本
   （`() => 1 could not be cloned.`），Aluka 为通用文案（`Function could not be cloned.`）；
   用例只判 `name`。

**本轮未做**：§待办 11 §6 的第 3 项（迭代器内部标记属性泄漏：`Object.keys(it)` 暴露
`_isArrayIterator` 等、`JSON.stringify(iter)` 暴露内部结构）——属表示层重构，改动半径
最大且与 `constructor`/`instanceof` 同根，仍待专项。

---

## 待办 14 · 内建原型读面收口（原型属性读 / `instanceof` / `constructor` / 迭代器标记）

> 承接 §待办 11 §6 第 3 项与 round7 登记的「原型方法属性读面」系统性缺口——此前
> 被判定为「改动半径最大、与 `constructor`/`instanceof` 同根」。本轮实测发现其**实际
> 缺口远小于登记印象**（20 项探针中仅 4 类差异），故在此收口。

### 开工前登记（目标 + 验收标准）

**缺陷（实测，改前基线；与 Node v22.3.0 对拍）**：

| 类别 | 改前实测 | Node |
|---|---|---|
| 原始类型原型方法属性读 | `typeof "abc".toUpperCase` → `undefined`；`(1).toFixed` 同 | `function` |
| `instanceof` | 仅 Array/Object/Error/RegExp 为 true；**Function/Map/Set/Date/Uint8Array/ArrayBuffer/Promise/Number 全为 false** | 全 true |
| `constructor` | `[].constructor.name` → `undefined`；`({}).constructor.name` → `undefined`；`(1).constructor`/`"a".constructor`/`(function f(){}).constructor`/`true.constructor` → **MISSING** | `Array`/`Object`/`Number`/`String`/`Function`/`Boolean` |
| 构造器 `name` | `Array.name` → `[function Function]` | `Array` |
| 迭代器标记泄漏 | `Object.keys([1,2].values())` → `["_isArrayIterator","_iterArray","next"]` | `[]` |

**根因（四类，均由 explorer 穷尽勘察确认）**：
1. `get_property` 的原型链遍历只覆盖 `Ordinary`/`Closure`/`NativeCtor`/`NativeFn`/`Array`
   五臂；`Value::Number`/`Boolean` 根本不是堆对象，`HeapObject::String`/`Symbol` 等
   变体无 `[[Prototype]]` 字段 → 链走不动，落到底部 `Ok(Value::Undefined)`。
   **注意**：surface **早已**把原型方法用 `define_proto_method` 挂成 `str_proto`/`num_proto`/
   `bool_proto`/`symbol_proto`/`fn_proto` 上的真实属性——缺的只是「读路径去查它们」。
2. `check_instanceof` 走原型链比 `r.prototype`，而 Map/Set/Promise/Date/TypedArray/
   ArrayBuffer/DataView/函数等实例用**无原型字段**的堆变体表示（仅 RegExp 有特判）。
3. 各原型上的 `constructor` 是 `surface` 挂的 **NativeFn 占位**（命名
   `"Array.prototype.constructor"`），非真构造器；且 `NativeFn`/`NativeCtor` 的 `name`
   字段未参与属性读。
4. 迭代器标记属性用 `set_property` 挂（**可枚举**）。

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | 原始值/无链接收者查对应原型面 | `typeof "abc".toUpperCase === "function"`、`(1).toFixed` 同；原始类型全部原型方法可读 |
| 2 | `instanceof` 内建兜底 | Function/Map/Set/Date/TypedArray 全族/ArrayBuffer/SharedArrayBuffer/DataView/Promise 均正确；**用户自定义 class 不误命中** |
| 3 | `constructor` 指向真构造器 + `name` 合成 | `[].constructor.name === "Array"`、`({}).constructor.name === "Object"`、`(1).constructor === Number`、`new Map().constructor === Map`、`Array.name === "Array"` |
| 4 | 迭代器标记改不可枚举 | `Object.keys([1,2].values())` → `[]`；`for...in` 不泄漏；`next`/`Symbol.iterator` 仍可读可用 |
| 5 | 门禁三连 + 零回归 | fmt/clippy/全量 + conformance 全绿 |

### 交付摘要

**5/5 达成。改动集中在 `property.rs`（读面 + instanceof）、`surface.rs`（constructor 挂接）、
`iter.rs`（标记转不可枚举）。**

- **`property.rs` 新增 `builtin_proto_of`**：按接收者类别给出对应原型单例
  （Number→num_proto、Boolean→bool_proto、String→str_proto、Symbol→symbol_proto、
  函数类→fn_proto），在 `get_property` 收尾前查一次自有属性——复用 surface 已挂的
  真实属性，**未新挂任何方法**。
- **`property.rs` 新增 `builtin_instance_of`**：按「构造器名 ↔ 堆变体」判定，
  仅在 `r` 为 `NativeCtor`（VM 自建构造器）时生效 → 用户 `class`（Closure）不会误命中；
  Map/Set 经 `is_set_instance` 区分（二者共用变体）、TypedArray 按
  `TypedKind::ctor_name()` 匹配。
- **`constructor` 真值**：`array_proto`/`object_prototype`/`fn_proto` 上的占位覆盖为真
  构造器（构造器单例在 `register_all` 之前已建好，时序安全）；Map/Set 在各自合成分支
  按实例登记取 `map_ctor`/`set_ctor`；原始值经 `resolve_global` 取 Number/String/Boolean/Symbol。
- **`name` 合成**：`NativeFn`/`NativeCtor` 的 `name` 字段参与属性读（先于 fn_proto 兜底）。
- **`Symbol.prototype.description`**：从堆字段合成真实取值（`Symbol("d").description === "d"`、
  `Symbol().description === undefined`）——此前落到 symbol_proto 占位 NativeFn。
- **迭代器标记**：`iter.rs` 11 处 `set_property` → `define_proto_method`（不可枚举 +
  登记 `non_enum`）；迭代结果对象的 `value`/`done` **保持可枚举**（Node 语义）。

**验收实测**（Node v22.3.0 逐行对拍）：

```
proto_probe.js（20 项原型读/constructor/instanceof/toStringTag） →  改前 8 行差异 → IDENTICAL
proto2_probe.js（32 项 String/Number 方法读 + 6 constructor + 12 instanceof）
                                                              →  改前 46 行差异 → 仅剩 1 项（见下）
name_probe.js（9 项构造器 name / 迭代器 keys）                  →  IDENTICAL
itershape_probe.js（迭代器内部标记泄漏）                        →  IDENTICAL
sym_probe.js（Symbol description）                            →  IDENTICAL
accept_probe / mapset / arrmethods / pop / samezero / insp / worker_map_probe
                                                              →  IDENTICAL（无回归）
sc2_probe（structuredClone）                                   →  仅剩已登记的 DataCloneError 文案差异
cargo fmt --all --check                                       →  exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                              →  exit 0，warnings=0 errors=0
cargo test --workspace --all-features                          →  exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                                               →  Result: 867/867 passed, 3 invalid
```

**隔离区偏差再判定**：`cases/gen/deviations/` 170 例中，**22 例现已与 Node 一致**
（改前约 13 例）。（按既有流程应由 `partition.py` 重新分区，本轮未手工搬移文件。）

**唯一未达成的验收项**：`new Number(1) instanceof Number` 仍为 false——explorer 已定位
为**明确死角**：Number 包装对象的堆表示未找到（无 `_isNumberObj`/`_boxed` 等标记，
`"Number"` 仅出现在 typeof 与 proto_ctor 分支）。属独立小专项，本轮登记。

**顺带修正一处过时 golden**：`core_semantics_test.rs::symbol_well_known_match_go`
期望 `desc: n/a`（录自 **Go 基线**，其 `Symbol.prototype.description` 未实现）。
Node 22 实测为 `has`——按项目「以 Node 22 LTS 为唯一权威 oracle」原则更新期望值，
并在用例文档注释中说明变更依据。

**本轮登记未做**（抽查确认均为**独立的其他缺口**，非回归）：
`"a".codePointAt` 等未实现的字符串方法、自定义 `Symbol.iterator` 生成器的展开、
`class B extends A` 的实例判定、`util.types.isPromise`、`Date.prototype.getTime`
（round7 已登记）、`new Number(1)` 包装对象。

---

## 待办 15 · `Date` 原型方法面补齐（`Date.prototype.*` + `Date.UTC` + tag/构造器）

> 承接 §待办 14 遗留清单第 1 项（M5.1「类型面全覆盖」曾因 Date 实例方法缺失而被迫
> 只判 `typeof`；`cases/gen/deviations/gen-date-matrix-*` 有 7 例分歧）。

### 开工前登记（目标 + 验收标准）

**缺陷（实测，改前基线；Node v22.3.0 对拍）**：`Date.now` / `Date.parse` 可用，
但**实例方法面几乎全空**：

| 表达式 | Node | Aluka 改前 |
|---|---|---|
| `new Date(0).getTime()` | `0` | `undefined` |
| `typeof new Date(0).getTime` | `function` | `undefined` |
| `new Date(0).toISOString()` | `"1970-01-01T00:00:00.000Z"` | `undefined` |
| `new Date(0).toJSON()` | `"1970-01-01T00:00:00.000Z"` | `TypeError` |
| `Date.UTC(1970,0,1)` | `0` | `TypeError` |
| `new Date(0).getFullYear()` | `1970` | `TypeError` |
| `Object.prototype.toString.call(new Date(0))` | `[object Date]` | `[object Object]` |
| `new Date(0).constructor.name` | `Date` | `Object` |
| `Object.keys(new Date(0))` | `[]` | `["_builtinNs","_isDate","_timeValue"]` |

**根因（两处叠加，已定位）**：
1. `crates/aluka-vm/src/builtins/global/mod.rs:142-152` 把 Date 方法 NativeFn 挂在
   **构造器** `date` 上（而非 `date_proto`）→ 属性读 `d.getTime` 走原型链找不到 → undefined；
   且只登记了 5 个方法名（getTime/valueOf/toISOString/toString/getTimezoneOffset），
   其余方法在分派表中根本不存在 → `TypeError: X is not a function`。
2. `crates/aluka-vm/src/builtins/global/date.rs:101-119` 的 `date_instance_method`
   从**接收者**当成 `NativeFn` 取方法名，而接收者是 Date 实例（`Ordinary`）→ 方法名恒为空
   → 命中 `_ => Ok(Value::Undefined)`。**故 `d.getTime()` 返回 undefined 而非报错。**

**分派机制（已确认）**：`_builtinNs = "Date"` 的实例，CALL_METHOD 走
`builtins/mod.rs:486` → 分派键 `"Date.{调用点方法名}"`。故实例方法须以 `"Date"` 模块名注册。

**约束（重要）**：仓库**无任何时间/时区依赖**（无 chrono/time/jiff），`unsafe` 被
deny 故无法 FFI 取系统时区。因此**本地时间类方法**（getFullYear/getHours/toString/
toLocale* 等）只能沿用现有「本地 = UTC（偏移 0）」的既定口径，并须显式登记偏离。

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | `date_instance_method` 方法名取真 | 用 `pending_native_name()` 末段；`d.getTime()` 得数值而非 undefined |
| 2 | 方法挂 `date_proto` + 实例原型指向它 | `typeof d.getTime === "function"`；`d.constructor === Date`；`d instanceof Date` |
| 3 | 补齐方法与分派表 | getTime/valueOf/toISOString/toJSON/getFullYear…getUTCMilliseconds/getTimezoneOffset/setTime/set*；`Date.UTC` |
| 4 | tag 与内部标记 | `Object.prototype.toString.call(d)` → `[object Date]`；`Object.keys(new Date(0))` → `[]`（`_builtinNs`/`_isDate`/`_timeValue` 改不可枚举） |
| 5 | 7 个 `gen-date-matrix` 分歧用例转一致 + 新增门禁语料 | 7 例 MATCH；新增 Date 用例 Node 侧多次运行同哈希且与 Aluka 逐字节一致 |
| 6 | 门禁三连 | fmt/clippy/全量 + conformance 全绿，零回归 |

### 交付摘要

**6/6 达成（第 5 项的"新增语料"由实现者完成，7 个分歧用例全转一致）。**

**根因两处（均已修）**：
1. `builtins/global/mod.rs` 原把方法 NativeFn 挂在**构造器**上（而非 `date_proto`），
   且只登记 5 个方法名 → 属性读走原型链找不到、其余方法在分派表中不存在；
2. `builtins/global/date.rs` 的 `date_instance_method` 从**接收者**当 `NativeFn` 取方法名，
   而接收者是 Date 实例（`Ordinary`）→ 方法名恒空 → 命中 `_ => Value::Undefined`
   （故 `d.getTime()` 返回 undefined 而非报错）。

**改法**：
- `date.rs`（+659）：重写为完整实例方法面 + `Date.UTC`，方法名统一取
  `pending_native_name()` 末段；模块文档登记时区口径与分派接线；
- `global/mod.rs`：方法挂 `date_proto`，**双键登记**（`"Date.{m}"` 供实例
  `_builtinNs` 分派 / `"Date.prototype.{m}"` 供 `.call` 形态）；
- `interpreter.rs`：新增 `Vm.date_proto` 字段；实例 `[[Prototype]]` 指向它
  （`constructor`/`instanceof` 随之成立）；
- **`gc.rs`：把 `date_proto` 登记为 GC 根**——新原型单例若漏登记，可能被回收成悬垂；
- `surface.rs`：`Object.prototype.toString` 的 tag 补 `_isDate` 分支 → `[object Date]`；
- Date 实例的内部槽（`_builtinNs`/`_isDate`/`_timeValue`）改为**不可枚举**
  → `Object.keys(new Date(0))` 由 `["_builtinNs","_isDate","_timeValue"]` 变为 `[]`。

**验收实测**：

```
【独立验收探针】date_utc_probe.js（52 项，只含 UTC 确定断言，本次评审自建）
   →  改前全部失效；改后 50/52 一致，仅剩 2 项为下述"新发现"（与 Date 无关）

【7 个分歧用例】gen-date-matrix-0001/0002/0003/0004/0006/0007/0010
   →  全部 MATCH（改前 0/7）

【新增门禁语料】tests/conformance/node22/cases/31-date-prototype.cjs
   →  PASS（含扩展年份边界 ±8.64e15、Date.UTC 越界/两位年/月份溢出、
      Invalid Date 三种形态、setUTC* 族、以及用 getTimezoneOffset 归一化
      使多参构造断言与时区无关）

cargo fmt --all --check                                       →  exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                              →  exit 0，warnings=0 errors=0
cargo test --workspace --all-features                          →  exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                                               →  Result: 868/868 passed, 3 invalid
                                                                 （867 + 新增 1；invalid 未增加）
```

**隔离区偏差再判定**：`cases/gen/deviations/` 170 例中 **30 例现已与 Node 一致**
（本轮 22 → 30；累计 13 → 22 → 30）。

**已登记偏离（本次最重要的一条，已在 `date.rs` 模块文档与用例注释双处登记）**：
仓库**无任何时间/时区依赖**（无 chrono/time/jiff），且 workspace 级 `unsafe_code = "deny"`
禁止 FFI 取系统时区，故：
- **UTC 类**（`getUTC*`/`toISOString`/`toJSON`/`toUTCString`/`Date.UTC`）与 Node **精确对齐**；
- **本地时间类**（`getFullYear`/`getHours`/`toString`/`toLocale*`/`getTimezoneOffset` 及
  `set*` 的本地语义）按**「本地 = UTC（偏移 0）」**计算：UTC 机器上与 Node 一致，
  本机（UTC+8）则相差一个偏移量——**预期内偏离，非回归**。`getTimezoneOffset()` 保持 `0`。
- 新增门禁语料**刻意不含本地时区断言**，以避免环境依赖导致的伪对拍。

**本轮新发现（独立验收探针所暴露，未修，另立）**：
`JSON.stringify` **完全不调用 `toJSON()`**——不只 Date，任何带 `toJSON` 的对象都失效：

| 表达式 | Node | Aluka |
|---|---|---|
| `JSON.stringify({toJSON(){return 1}})` | `1` | `{}` |
| `JSON.stringify({a:{toJSON(){return "x"}}})` | `{"a":"x"}` | `{"a":{}}` |
| `JSON.stringify([{toJSON(){return 7}}])` | `[7]` | `[{}]` |
| `JSON.stringify({d:new Date(0)})` | `{"d":"1970-01-01T00:00:00.000Z"}` | `{"d":{}}` |

这是**比 Date 更广**的 JSON 语义缺口（`toJSON` 是生态常用协议）。未在本轮修的原因是
`prims.rs::json_write` 为 `&self` 且持有堆借用，改为 `&mut self` 需重构 JSON 热路径，
风险与收益不匹配，故登记另立专项。

---

## 待办 16 · `JSON.stringify` 支持 `toJSON()`（JSON 序列化协议补齐）

> 来源：§待办 15 的独立验收探针暴露（非 Date 特有）。`toJSON` 是 ES 规范
> `SerializeJSONProperty` 的正式协议，也是生态常用扩展点（Date/Moment/各类 ORM 都靠它）。

### 开工前登记（目标 + 验收标准）

**缺陷（实测，改前基线）**：

| 表达式 | Node | Aluka 改前 |
|---|---|---|
| `JSON.stringify({toJSON(){return 1}})` | `1` | `{}` |
| `JSON.stringify({a:{toJSON(){return "x"}}})` | `{"a":"x"}` | `{"a":{}}` |
| `JSON.stringify([{toJSON(){return 7}}])` | `[7]` | `[{}]` |
| `JSON.stringify({d:new Date(0)})` | `{"d":"1970-01-01T00:00:00.000Z"}` | `{"d":{}}` |
| `JSON.stringify({toJSON(){return undefined}})` | `undefined` | `{}` |

**根因**：`prims.rs::json_write` 直接按堆变体写值，**从不查询 `toJSON` 属性**。

**根因的改造难点（决定改法）**：`json_write` 现为 `&self`，其函数体在
`self.heap.get(...)` 的**共享借用**内递归。而要调用 `toJSON` 需
`get_property`/`invoke_callable`（`&mut self`）→ 借用冲突。故须重构为 `&mut self`
并把「先快照、后递归」落实到各分支（数组按**下标逐次读取**而非持有 `elements` 借用、
对象沿用既有 `own_entries` 快照、字符串按需 clone）。

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | `json_write` 改 `&mut self` + 传 `key` 参数 | 编译通过、无借用冲突；递归不再持有堆借用跨 `&mut self` 调用 |
| 2 | 按规范实现 `SerializeJSONProperty` 的 `toJSON` 步骤 | 值（或其包装）若有**可调用**的 `toJSON` 属性，则以**属性键**为实参调用，并用返回值继续序列化；键规则：根为 `""`、对象属性为属性名、数组元素为下标字符串 |
| 3 | 边界 | `toJSON` 非函数 → 忽略；`toJSON` 返回 `undefined` → 该处输出 `null`（对象属性则**整条省略**）；`toJSON` 抛错 → 错误向外传播；根值 `toJSON` 返回 `undefined` → 整体返回 `undefined` |
| 4 | 与既有行为不回归 | `JSON.stringify(Promise.resolve(1))` 仍 `{}`；`undefined`/函数/符号顶层仍返回 `undefined`；数组内 `undefined`/函数仍写 `null`；循环引用仍 `null`（`seen` 守卫）；`Map`/`Set` 仍 `{}` |
| 5 | 新增门禁语料 | `32-json-tojson.cjs`：Node 侧多次运行同哈希、与 Aluka 逐字节一致；`invalid` 不增加 |
| 6 | 门禁三连 + Date 用例 | fmt/clippy/全量 + conformance 全绿；`date_utc_probe.js` 的 2 项 stringify 差异转为一致 |

### 交付摘要

**6/6 达成。**

**改法（`prims.rs` 单文件）**：

- **`json_write` 重构为 `&mut self` + `Result`**：新增局部 `Kind` 枚举，把堆变体
  先快照成 **owned 数据**（`Text(String)` / `Arr(len)` / `Obj(Vec<(String,Value)>)`），
  借用即结束，故递归时可安全取 `&mut self`（`toJSON` 可能是用户函数）。
  数组分支改为**按下标逐次读取**（不持有 `elements` 借用）；对象分支沿用既有
  `own_entries` 快照并把「剔除不可序列化值」的过滤下移到 for 循环内。
- **新增 `Vm::apply_to_json(value, key)`**：按规范 `SerializeJSONProperty` 第 2 步——
  值为对象且 `toJSON` **可调用**（经 `get_property` 走原型链，与 `GetV` 一致）时，
  **以属性键为唯一实参**调用并用返回值继续序列化；Proxy 亦视为可调用（走 apply trap）。
- **键在「取用处」逐次应用一次**（而非在 `json_write` 内部重复应用）：根为 `""`、
  对象属性为属性名、数组元素为下标串——规范如此，且避免 `toJSON` 被调用两次的
  可见副作用（用例 `call-count-root`/`call-count-prop` 断言为 1）。
  > 与原计划的差异：**未**给 `json_write` 增传 `key` 参数——键在调用点已消费，
  > 函数内无需再知键，故签名改动更小（`&mut self` + `Result`）。
- 顶层：`json_stringify` 先对根值应用 `toJSON`，再做「不可序列化」判定，
  故 `{toJSON(){return undefined}}` 正确得到整体 `undefined`。

**验收实测**：

```
【专项探针】tojson2_probe.js（38 项，本会话自建）
   →  37/38 与 Node 一致；唯一差异为「循环引用」——
      Node 抛 TypeError、本实现按既有登记降级为 null
      （已用 git stash 对照验证：该守卫在改动前即存在，非本次引入）

【既有探针回归】date_utc_probe.js（52 项，含先前 2 项 stringify 差异）→ IDENTICAL
              tojson_probe.js（4 项）                              → IDENTICAL

【新增门禁语料】tests/conformance/node22/cases/32-json-tojson.cjs（37 行）
   →  Node 侧 5/5 运行同哈希；与 Aluka 逐字节一致；PASS

cargo fmt --all --check                                       →  exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                              →  exit 0，warnings=0 errors=0
cargo test --workspace --all-features -- --test-threads=1      →  exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                                               →  Result: 869/869 passed, 3 invalid
                                                                 （868 + 新增 1；invalid 未增加）
```

**隔离区偏差再判定**：`cases/gen/deviations/` 170 例中 **34 例现已与 Node 一致**
（累计 13 → 22 → 30 → 34）。其中 `gen-object-json-0010.cjs`
（`JSON.stringify({toJSON:()=>"j"})`）由本次修复直接转 MATCH。

**本轮排查的一个重要发现（环境/CI 层面，非代码缺陷）**：

`cargo test --workspace --all-features` **默认并行**时，
`builtins_phase4_zlib_test` 的 2 个重型用例（`zlib_large_input_roundtrip_lengths`、
`zlib_zstd_roundtrip`）会失败：探针用 `for (i<20000) big += "0123456789"` 累加到
200KB，**字符串不可变 → 累计产生约 2GB 瞬时垃圾**，多个此类进程并发即耗尽内存，
报 `memory allocation of N bytes failed`。

- **判定依据（决定性）**：`git stash` 掉本次改动、在未改动的 HEAD 上重建后，
  同样两个用例以同样错误失败 → **与本次改动无关**；
- 独立跑 `aluka.exe` / `alukac+aluvm` 该探针均**成功**；`--test-threads=1` 下
  18/18 全过 → 确认是**并行内存压力**而非语义问题；
- 故本轮全量门禁以**单线程**取证（586/0/1 与基线一致）。
  建议后续把这两个用例的输入规模收敛（或标记为串行），以免在内存受限的
  CI/开发机上产生假失败——已登记为独立跟进项。

---

## 待办 17 · zlib 重型用例内存收敛（恢复默认并行下的干净门禁）

> 承接 §待办 16 的排查发现。**测试基建问题，非引擎缺陷** —— 但它会给全量门禁
> `cargo test --workspace --all-features`（默认并行）制造**假失败**，必须修掉。

### 开工前登记（目标 + 验收标准）

**缺陷（实测）**：`builtins_phase4_zlib_test.rs` 的两个用例探针用
`for (var i = 0; i < 20000; i++) { big += "0123456789"; }` 构造 200000 字节输入。
JS 字符串不可变 → 每轮都产生新串，累计分配
`10 × (1+2+…+20000) ≈ 2.0 GB` 瞬时垃圾（且紧循环内 GC 未介入）：

| 度量 | 实测 |
|---|---|
| 单进程峰值工作集（原追加版） | **2052.2 MB** |
| 并行跑多个该类进程 | 内存耗尽 → `memory allocation of N bytes failed` |
| 单线程/单独运行 | 通过（故此前从未暴露） |

**目标**：**保持 200000 字节 payload 与全部断言不变**，仅消除二次方垃圾。
改用**倍增构造**（`while (len < 200000) big = big + big; big = big.slice(0, 200000)`）：
分配量降为 `10+20+…+327680 ≈ 655 KB` 级别。

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | 两处探针改倍增构造 | 与追加版**内容完全相同**（Node 侧 `a === b` 为 true，均 200000 字节）；峰值内存显著下降 |
| 2 | 两个用例断言**不变** | 仍断言 `zstd roundtrip 帧格式\n200000` 与 `200000×6`；用例语义（同长度输入的各格式往返）不减弱 |
| 3 | 恢复默认并行门禁 | 不带 `--test-threads=1` 的 `cargo test --workspace --all-features` 全绿 |

### 交付摘要

**3/3 达成。**

**改法（`crates/aluka-cli/tests/builtins_phase4_zlib_test.rs` 两处探针）**：
把二次方的逐次追加改为**倍增构造**——

```js
// 改前（累计分配 ≈ 2.0 GB）
var big = "";
for (var i = 0; i < 20000; i++) { big += "0123456789"; }
// 改后（累计分配 ≈ 655 KB）
var big = "0123456789";
while (big.length < 200000) { big = big + big; }
big = big.slice(0, 200000);
```

**双双验证（内容未变、内存骤降）**：

| 度量 | 改前 | 改后 |
|---|---|---|
| 与追加版内容是否一致（Node 侧 `a === b`） | — | **true**（均 200000 字节） |
| 单进程峰值工作集 | **2052.2 MB** | **103.0 MB** |
| 用例断言 | `zstd roundtrip 帧格式\n200000`、`200000×6` | **完全不变**（语义未减弱） |
| `builtins_phase4_zlib_test`（默认并行） | 2 failed | **18/18 passed** |

**关键验收（恢复默认并行口径）**：

```
cargo test --workspace --all-features            →  exit 0
                                                   586 passed / 0 failed / 1 ignored
cargo test -p aluka-cli --all-features --test builtins_phase4_zlib_test
                                                 →  18 passed / 0 failed（4.77s）
```

即 §待办 16 里被迫以 `--test-threads=1` 取证的口径**已恢复**：现在默认并行下全量门禁
干净通过，不再有 `memory allocation ... failed` 假失败。

**性质说明**：这是**测试基建**修复，未改动引擎任何行为；两个用例仍以 200000 字节
输入验证各压缩格式往返，覆盖度不变。

---

## 待办 18 · 用户类继承的 `instanceof`（`class B extends A`）

> 承接 §待办 14 遗留清单第 2 项：`cases/gen/deviations/gen-class-proto-0001..0010` 有 8 例
> 偏差。§待办 14 建立的 `builtin_instance_of` 只处理**内建构造器**（NativeCtor），
> 对用户 `class`（Closure）**有意不生效**以防误命中，故继承判定仍缺。

### 开工前登记（目标 + 验收标准）

**缺陷（待实测确认基线）**：`class A {} class B extends A {} new B() instanceof A`
在 Node 为 `true`，预期 Aluka 为 `false`（`get_prototype` 对 Closure 返回 `proto`，
但 `class extends` 是否写入该字段、以及 `A.prototype` 是否挂对，需实测）。

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | 实测 8 个 `gen-class-proto` 用例基线 | 逐条记录 node vs aluka |
| 2 | 修用户类继承链的 `instanceof` | `new B() instanceof A` → `true`；`new A() instanceof B` → `false`；单层类 `new A() instanceof A` → `true`；**内建判定不回归** |
| 3 | 门禁语料 + 三连 | 新增/扩展用例 Node 侧确定且一致；fmt/clippy/全量/conformance 全绿 |

### 交付摘要

**3/3 达成——但根因与开工假设完全不同，实际修复的是更严重的编译器缺口。**

**开工假设被实测推翻**：原以为这是 `instanceof` 判定缺陷。实测（逐构造单独成文件——
因为一个不支持的语法会让**整个文件**解析失败、污染批量对比）后发现：

| 构造 | 改前 | 结论 |
|---|---|---|
| 顶层 `class A { m(){} }` + `new A().m()` | 正常 | 顶层无问题 |
| 顶层 `class B extends A` + `instanceof`/`super`/覆盖/三级链 | 全部正常 | **继承与 `instanceof` 本身没问题** |
| **函数/箭头/IIFE 体内** `class A { m(){} }` | `typeof A` → undefined、`new A().m` → undefined、`new B() instanceof A` → false | **真正的缺口** |
| 解析器：class 字段、`static` 方法/字段/块、getter/setter、`#p`、class 表达式作操作数 | 解析错误 | 另一族（parser/lexer），本轮未动 |

`gen-class-proto-0002/0006/0009`「看似是 `instanceof` 问题」，实为该生成器把用例包在
**箭头 IIFE** 里 —— 触发的是作用域缺口。

**关键误导点**：`new A()` 在类未绑定时**居然成功**，一度让人以为绑定存在。实测
`new TotallyUndeclared()` 同样不报错（Node 抛 `ReferenceError`）——`new <未声明标识符>`
存在宽松回退。故 `new A()` 可用是**假信号**。

**根因（精确定位）**：
- `crates/aluka-compiler/src/codegen.rs:746`：`Stmt::Function(_) | Stmt::Class { .. } => {}`
  —— 常规语句编译路径把类声明当**空操作**丢弃，注释称「在 compile_module 中提取」；
- 而提取只发生在 **`compile`（模块顶层语句）**：`module.rs:454` 有完整的 `MakeClass`
  装配（含 `extends` 的 `__home_ctor__`/`__home_proto__` 槽）；
- **嵌套函数**路径 `compile_function_with_parent`（`module.rs:1063` 的语句循环）只特判
  `Stmt::Function`，`Stmt::Class` 落到 `compile_stmt` → 被丢弃。该路径的**槽位预注册**
  已含 `Stmt::Class`（`module.rs:996`），只缺**发射**。

**改法（`module.rs` 单点、镜像顶层）**：在嵌套语句循环新增 `Stmt::Class` 分支，复用
`self.compile_class(...)` + `Op::MakeClass` + `StoreLocal`，并对 `extends` 同样装配父类
构造器与原型槽。约 60 行；未改动顶层路径与 VM。

**验收实测**：

```
【逐构造矩阵】33 项（classmat/classscope/classdiag 系列，本会话自建）
   改前：函数作用域内 9 项失败
   改后：函数/箭头/IIFE/嵌套函数 全部与 Node 一致；diag4 四行与 Node 完全一致

【8 个 gen-class-proto 分歧用例】4 个转 MATCH（0001/0002/0006/0009 —— 作用域族）
   剩余 4 个（0003 getter / 0004 static 方法 / 0005 私有字段 / 0010 static 块）
   为 parser/lexer 缺口，根因不同，本轮未动

【新增门禁语料】tests/conformance/node22/cases/33-class-scope.cjs（23 行）
   → Node 侧 5/5 同哈希；与 Aluka 逐字节一致；PASS

cargo fmt --all --check                                  → exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                         → exit 0，warnings=0 errors=0
cargo test --workspace --all-features（默认并行）          → exit 0，586 passed / 0 failed / 1 ignored
conformance 全量                                          → Result: 870/870 passed, 3 invalid
                                                            （869 + 新增 1；invalid 未增加）
```

**隔离区偏差再判定**：`cases/gen/deviations/` 170 例中 **38 例现已与 Node 一致**
（累计 13 → 22 → 30 → 34 → 38）。

**本轮登记、未做**（均经实测确认根因不同）：
1. **`class` 在裸块 `{ class A {} }` 内仍失效** —— 走 `Stmt::Block`（codegen.rs:202）
   递归 `compile_stmt` → 同被丢弃；修复需连带块的 let/const 遮蔽簿记
   （`scope_shadow_log`），风险高于函数路径，故本轮只修函数族（真实代码影响面最大者）；
2. **解析器缺口**：class 字段、`static` 方法/字段/块、getter/setter、私有字段、
   class 表达式作操作数 —— 属 aluka-parser 改造；
3. **`new <未声明标识符>` 宽松回退**：不报错（Node 抛 `ReferenceError`），会掩盖真实
   绑定错误 —— 本轮由它产生过误导，建议优先复核；
4. **类名 `A.name` 返回 `A_constructor`**（Node 为 `A`）—— `compile_class` 里
   `format!("{name}_constructor")` 的默认构造器命名泄漏到 `name` 字段。
