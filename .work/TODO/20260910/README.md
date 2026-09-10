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
