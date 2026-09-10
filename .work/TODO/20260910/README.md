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
