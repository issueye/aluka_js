# aluka Rust 重构——详细开发计划（MVP 里程碑 + 并行轨道）

> 状态：规划稿（v0.3）｜日期：2026-09-03｜依赖：rust-reimplementation-plan.md（架构与阶段）
> 关联决策：docs/adr/jvm-style-bytecode-architecture.md（ISA 契约，已采纳）
> 原则：strangler 逐子系统替换；conformance 套件为唯一行为仲裁；每里程碑有可演示交付物
> 路径约定：见 rust-reimplementation-plan.md 头部（裸写的 `internal/…`、`tests/…` 等
> 相对 `aluka_g/`；`crates/…` 相对 `aluka_r/`；带 `docs/`、`aluka_g/`、`aluka_r/` 前缀的相对仓库根）

---

## 0. 总览

- 终局是**Rust 版取代 Go 版**（不是长期双实现）。两个硬前提：**完全兼容 JS/TS
  语法**（M2 达成）与 **ISA 字节码契约落地**（M4 达成），全部退役门禁见 §8。
- 架构切成 **alukac 前端 + aluvm 后端 + ISA 契约**三块（JVM 式）。这决定了本
  计划最重要的并行结构：前端与后端**互不阻塞**，各以对面的 Go 实现做对照。
- 5 条并行轨道（F/A1/A2/B/C）+ 2 条支撑流（D/E）。公共前置是 **M0**：ISA 规范化
  + verifier 强化 + golden 语料 + GC 选型。
- MVP 定义分四档（§3），每档可独立对外演示。
- 假设人力：5-7 名 Rust 工程师 + 现有作者做仲裁/Go 侧对接；可增减按"轨道×人月"折算。
- 月 = 有效 4 周；里程碑验收 = 演示 + conformance 数字 + 性能门，不达标不进入下一里程碑（可裁剪范围但不可裁剪验收）。

---

## 1. 并行轨道与依赖矩阵

### 轨道总览

ISA 契约（轨道 F）是并行化的**总闸门**：它一落地，前端（A2）与后端（A1）
就只经字节码耦合，可完全独立推进。这是相对 v0.2 计划最重要的结构变化——
原先 A 轨内部 `parser→compiler→vm` 是串行的。

| 轨道 | 内容 | 依赖 | 并行性 |
|------|------|------|--------|
| **F ISA 契约** | 规范化 `bytecode-spec`、verifier 强化到"通过即安全"、golden 语料 | 无（Go 版即可做） | **最先启动**，其余轨道的前置 |
| **A1 aluvm 后端** | Value/Shape/GC/加载器/verifier/VM | F 的规范 + golden 语料 | 与 A2 完全并行：**输入用 Go 前端产出的字节码**，不必等 Rust 前端 |
| **A2 alukac 前端** | lexer/parser/AST/TS 剥离/JSX lowering/字节码生成 | F 的规范 | 与 A1 完全并行：**产物喂 Go VM 验证**，不必等 Rust 后端 |
| **B 内置库移植** | node:* 58 模块 + Web API + Aluka API | A1 的 `Value`/`Context` API（冻结版） | 与 A1/A2 并行：先 FFI 到 Go builtin 过渡，API 冻结后逐模块 Rust 化 |
| **C 工具链** | pkgmanager/bundler/Vue SFC/CLI | 编译产物规范（非引擎内部 API） | 完全独立；早期即可用 Go CLI 壳 |
| **D 测试与一致性** | conformance 迁移、jitdiff/fuzz 移植、bench 基线、双向字节码对拍 | 随各轨进展持续 | 恒并行，串起各轨验收 |
| **E JIT 后端** | Quick IR → Cranelift / 自研 amd64 | F 的 verifier + A1 的 VM 稳定 | 晚启动（M5，M0 期先做 Value/GC 原型），可与 C/D 并行 |

### 依赖阻塞关系

| 轨道 | 被谁阻塞 | 说明 |
|------|---------|------|
| F | 无 | Go 版上即可完成，是全局起点 |
| A1 | F | 需要 ISA 规范 + golden 语料才能实现加载器与 verifier |
| A2 | F | 需要 ISA 规范才能确定字节码生成目标 |
| B | A1（`Value`/`Context` API 冻结） | 仅需 API 形状，不需 VM 完成 |
| C | 无 | 只依赖编译产物规范（CLI 层），与引擎内部无关 |
| D | 无（消费各轨产物） | 恒并行；各轨的验收由它执行 |
| E | F（verifier 强度） + A1（VM 稳定） | JIT 信任 verifier 结论来省略运行期检查 |

**关键洞察**：A1 与 A2 之间**没有**阻塞关系。两侧各用对面的 Go 实现做对照，
这把原计划的串行关键路径缩短了一整个前端工期（约 3 个月）。

---

## 2. 里程碑分解（M0→M7，含任务清单与验收）

里程碑与 `rust-reimplementation-plan.md` §4 的 8 个阶段一一对应；此处给出
任务级拆解与轨道归属。

### M0：ISA 规范化 + 技术原型（2 个月，F 轨主力 + A1/E 联合）

**目的**：解除两个最大风险（ISA 契约边界、GC 选型），打开 A1/A2 的并行闸门。

任务（F 轨——ISA 契约）：
- [ ] 把 `docs/bytecode-spec.md` 从"实现说明"提升为**规范**：每条指令的操作数
      编码、栈效果、异常条件、可观察副作用都写死
- [ ] 校验规则成文：跨块栈深合流、try 表结构合法性、跳转目标边界、常量池类型
- [ ] **Go 版 verifier 强化到"通过即安全"**（对 Go 版本身也是净安全收益，
      且 Rust JIT 会信任 verifier 结论来省略运行期检查）
- [ ] 扩展指令的**能力位协商**约定：header 声明，未知位必须拒绝加载
- [ ] **golden 字节码语料 ≥200 例**（Go 前端产出）：全指令覆盖 + 每例期望输出

任务（A1/E 轨——技术原型）：
- [ ] Value 表示原型：NaN-box u64 vs enum，微基准（复制/算数/分发）
- [ ] GC 原型 ×2：分代标记-清除（bump 年轻代 + 卡表）；RC + 循环回收
- [ ] 以 fib30 / 对象创建循环为负载，输出两份性能报告
- [ ] Shape/IC 原型
- [ ] 冻结 `aluka-core` 的 `Value/Heap/Shape` 公共 API（B 轨的输入）

验收：ISA 规范可据以**独立实现前后端**（第三方读规范即可写前端）；golden 语料
≥200 例覆盖全指令；M0 报告选定 GC 策略 + API 冻结；回归到 Go 版基线对照。

### M1：MVP-1 aluvm 吃 Go 前端字节码（3 个月，A1 轨主力）

**目的**：后端骨架跑通。**关键并行收益**——输入用 Go 前端产出的 `.aluc`，
不必等 Rust 前端，M1 关键路径因此缩短一整个前端工期（约 3 个月）。

任务（A1 aluvm）：
- [ ] Value + 对象盒 + Shape/IC（M0 定案的表示）
- [ ] 字节码加载器 + verifier（Rust 侧实现 M0 规范）
- [ ] Tier 0 解释器全指令：算术/比较/控制流/闭包/对象字面量/异常
- [ ] CJS 模块最小集（require + 循环依赖）
- [ ] 内置：console/process/fs 最小（path read/write）
- [ ] `aluvm run hello.aluc` + `--version` + 退出码语义

任务（A2 alukac，同期启动、不阻塞 A1）：
- [ ] lexer + parser 骨架：ES5 全量 + ES2015 主干语法
- [ ] **产物喂 Go VM 验证**（反向对拍，同样不必等 Rust 后端）

任务（D 轨）：
- [ ] golden 语料回归脚本（Rust VM vs Go VM 逐例对拍）
- [ ] bench 基线脚本接入（沿用 Go 版 diff 方法学：交替执行、冷却、min-of-N）

交付物：`aluvm` 二进制跑通 hello/fib30/简单文件 IO（字节码来自 Go 前端）；
`alukac`（部分语法）产物可被 Go VM 执行。

验收：golden 语料 **100% 行为一致**；node conformance ≥8/11（经 Go 前端编译）；
fib30 ≤ Go 版 2×（40ms 级，暂不追求快）。

### M2：MVP-2 全语言 + 模块（4 个月，A1+A2+B 汇合）—— **终局前提 1**

**目的**：JS 语言全量 + TS 剥离 + Node 兼容可跑真实包。这是"完全兼容 JS/TS
语法"的达成点。

任务（A2 alukac）：
- [ ] ES2015-2024 全语法解析（class/生成器/async/TLA/Proxy/BigInt/装饰器）
- [ ] TS 注解剥离 + TSX/JSX lowering（对齐 Go 版 parser 层剥离策略）
- [ ] 字节码生成 + 优化 pass（常量折叠/不可达删除/融合/跳转穿透）
- [ ] `alukac` 二进制：产出 `.aluc`/`.alua`

任务（A1 aluvm）：
- [ ] ES2024 全量**语义**：class/生成器/async/迭代器/Proxy invariants/BigInt
- [ ] RegExp 双路引擎（RE2 风格 + 回溯 fallback，UTF-16 索引 + 预算护栏）
- [ ] 模块系统完整：ESM/CJS/exports 条件（实例级）/imports/循环/TLA
- [ ] 字节码缓存 + FormatVersion 校验

任务（B 轨首交）：
- [ ] FFI 桥：Rust VM → Go builtin（句柄表，对象生命周期以 Rust 侧为准）
- [ ] node:* 高频 15 模块直接可用（fs/url/path/events/util/assert/stream/
      buffer/timers/os/string_decoder/querystring/module/process/console）

任务（D 轨——双向字节码对拍，本里程碑的核心门禁）：
- [ ] Rust 前端产物喂 Go VM；Go 前端产物喂 aluvm；四象限全绿
- [ ] test262 子集接入，与 Go 版同分比对

交付物：跑通 `npm i express && aluka app.js`（express 真实 HTTP 链路）；
TS/JSX 样例转译正确。

验收（**对应终局前提 1**）：node22 17/17 + express conformance + webapi 套件绿；
test262 子集与 Go 版**同分或更高**；Rust 前端与 Go 前端对同一源码产出**语义
等价**字节码（允许指令序列不同，可观察行为必须一致）。

### M3：MVP-3 性能对齐（GC/数组专项，3 个月，A1+B 深化）

**目的**：分配与内存模型上量——这是整个重构的原始动机（Go 版四个 ADR 证明
的结构性上限）。

任务：
- [ ] GC 落地（M0 选定方案完整实现：写屏障/卡表/根集/shadow stack）
- [ ] 数字内联槽、packed elements（`Vec<f64>` 数组）、array holes 位图
- [ ] Array 方法批量快路径（push/map/filter/reduce）
- [ ] 字符串 rope + 拼接优化（对齐 Go 版 ME-1 结论）
- [ ] B 轨：剩余内置模块 FFI 补全 + 高频模块 Rust 化（crypto/zlib/http 网络层）

交付物：gcPressure 形态（500K 循环、1% 保留）与 node 对拍。

验收：**gcPressure ≤ node 3×**（Go 版 21×）；arrayPush ≤ node 5×；fib30 ≤ node 3×。

### M4：ISA 发布契约（1.5 个月，F 轨收口）—— **终局前提 2**

**目的**：字节码从"引擎内部缓存"升格为**平台发布契约**。这是"新语法无缝
接入"的达成点。

任务：
- [ ] 定 `.aluc`/`.alua` 二进制格式（含调试信息段与剥离选项）
- [ ] 拆 `alukac`/`aluvm` 两个二进制；`aluka` 退化为便利壳（等价 `java` 直接跑源码）
- [ ] 决定 `eval`/`new Function` 策略（内嵌编译器 or 受限模式）与
      `Function.prototype.toString` 合规性（源片段 or 降级）
- [ ] 承诺兼容窗口 + 核心 ISA 版本递增权限（架构评审门）
- [ ] **写一个玩具 DSL 前端**（例如极简 Lisp 或类 Python 缩进语法），只读 ISA
      规范、不碰后端代码，产出可执行字节码

验收（**对应终局前提 2**）：玩具 DSL 前端在 aluvm 上跑通，全程未修改后端——
这是"新语法零成本接入"的可执行证据，不是纸面承诺。

### M5：JIT（3.5 个月，E 轨主力）

**目的**：解释器之上的性能主链。

任务：
- [ ] Quick IR（常量折叠/store-load 消除/不可达删除）→ Cranelift（低风险优先）
      或自研 amd64 后端
- [ ] 属性 PIC（shape guard）、去优化恢复、栈映射
- [ ] 闭包/upvalue/数组下标等已有特化等价物（对齐 Go 版 jit-coverage-matrix）
- [ ] 移植 jitdiff 生成式差分 + 5 个 fuzz target（保持"三 tier 零失配"传统）

交付物：propAccess/callOverhead 与 node 对拍表。

验收：propAccess ≤ node 5×；closureCall ≤ node 5×；jitdiff ≥3 千例零失配。

> **口径纪律（20260905 补）**：node 侧必须**预热到优化层**（TurboFan）后计时。
> 未预热的 V8（Ignition/`--no-opt`）在这两个负载上慢 20 余倍，用它当基线会把
> 本项虚假标绿。此外「与 aluka 基准逐指令同构」的 JS 写法会被 V8 优化成等价
> 空循环（callee 内联 + 对象逃逸分析 + 循环不变量提升，实测 0.45 ns/iter），
> 必须另备阻断这些归约的变体。脚本与三模式对照见
> `.work/evidence/20260905/node-jitbench.js` 与 `m5-j2-pic-report.md` §会话 8。

### M6：工具链迁移（2.5 个月，C 轨主力，与 M5 并行）

任务：
- [ ] pkgmanager：registry/semver/hoisting/lockfile/workspace
- [ ] bundler：compile 产物（payload+manifest+footer）与 web 打包
      （graph/shake/minify/chunk/UMD）
- [ ] Vue SFC subset 后端 + official（FFI 调 Vue compiler-sfc 或独立进程）
- [ ] GUI 桥（WebView2/WKWebView）+ dev/watch

交付物：`aluka build --compile` 与 `--target=web` 对 Go 版产物 diff 一致。

验收：build/webbuild conformance 全绿（24/24、13/13）；产物可反编译预览一致。

### M7：Go 版退役（1.5 个月）

任务：
- [ ] AIP/IPC、monitor、heap snapshot、inspector CDP、node:test、repl 补齐
- [ ] `pkg/aluka` 的等价 C ABI / Rust 嵌入 API，现有嵌入方迁移指南
- [ ] 全 conformance 终检 + Miri/unsafe 审计 + 性能终检表
- [ ] 文档/ADR 迁移（新增 Rust 架构决策记录）
- [ ] **八项退役门禁逐项签核**（§8）
- [ ] Go 版打最后一个维护 tag，仓库主线切到 Rust 实现

验收：§8 八项门禁全过。未全过则 Go 版保持可构建可发布，Rust 版以 **preview**
身份并行分发——不做"两版都是正式版"的承诺。

---

## 3. MVP 定义（横切片，供演示/评审/发布）

- **MVP-1**（M1 末）：`aluvm hello.aluc` —— 后端可独立执行 Go 前端产出的字节码，
  静态单二进制、零依赖。演示点：**两个实现的字节码互通**。
- **MVP-2**（M2 末）：`npm i` 生态可玩 —— express/React SSR 可跑，TS/JSX 编译。
  演示点：**终局前提 1 达成**（语法完全兼容）。
- **MVP-3**（M3 末）：性能基准达标 —— gcPressure ≤3× node（Go 版 21×）。
  演示点：**重构的原始动机被兑现**。
- **MVP-4**（M4 末）：ISA 契约发布 —— 玩具 DSL 前端零改后端跑通。
  演示点：**终局前提 2 达成**（新语法无缝接入）。

每 MVP 附带：conformance 报告 + 性能对拍表 + 5 分钟演示脚本。

---

## 4. Gantt 时间线（并行视图，月份 = 有效 4 周）

```
Month       1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20
F  ISA    [-M0-]..............................[-M4-]............[-M7-]
A1 aluvm  ......[--M1---][----M2----][--M3---]........................
A2 alukac ......[-parse-][-M2 lang--].................................
B  node:* .........[--FFI--][15 mods][-rustify--].....................
C  tools  ................................................[--M6---]...
D  tests  [-----golden / bytecode bidi / test262 / bench / fuzz------]
E  jit    [-pr-].................................[----M5----].........
```

里程碑结束月：M0 = 2、M1 = 5、M2 = 9、M3 = 12、M4 = 14、M5 = 17、M6 = 19、
M7 = 20（与 plan §8 的 +2/+5/+9/+12/+13.5/+17/+19/+20 一致，图上取整到整月）。
MVP-1…MVP-4 分别交付于 M1/M2/M3/M4 末（§3）。

工期串行相加为 21 月，日历工期 20 月——差额来自 M4/M5 首尾重叠（分属 F 轨与
E 轨）。M0 的 GC/Value 原型由后来转入 E 轨的人力承担，故 E 轨在第 1-2 月已有
投入（图中 `pr`）。

并行关键（三条独立的关键路径缩短）：

1. **A1 与 A2 互不阻塞**：A1 吃 Go 前端字节码，A2 产物喂 Go VM。这是 ISA 契约
   带来的最大收益——原计划 `parser→compiler→vm` 串行，现在两侧同时起步，
   M1 的关键路径缩短一整个前端工期（约 3 个月）。
2. **B 的 FFI 过渡与 A1/A2 的 M1-M2 同时进行**：B 只需 M0 冻结的 `Value` API，
   不需要 VM 完成。
3. **C 只依赖"编译产物规范"而非引擎内部 API**：可任意时点插入，此处排在 M6
   是人力排布结果，不是依赖约束。

人力分配（5-7 人）：F×0.5-1（作者主导）、A1×2、A2×1-2、B×1-2、C×1、
D×0.5-1、E×1（M5 起从 A1 转入）。

---

## 5. 每周节奏与门禁

- 周一：轨道站会（阻塞、风险、diff 门禁项）
- 周五：CI 门禁——`cargo test` + golden 语料对拍 + conformance 快照（node 3 条核心）
  + bench 冒烟
- 每里程碑：冻结该里程碑验收表，不达成不许进入下一里程碑（范围可裁剪，验收不可裁剪）
- 每两周：**双向字节码对拍**——Rust 前端产物喂 Go VM、Go 前端产物喂 aluvm，
  四象限行为必须一致（性能除外）

这里的"双向对拍"是 v0.2 计划里没有的新门禁，也是 ISA 契约的实际收益：缺陷定位
从"整链对拍"细化为"字节码层 + 行为层"两级——前端 bug 表现为字节码差异，后端
bug 表现为同一字节码执行结果差异。

---

## 6. 并行风险与缓解

| 风险 | 缓解 |
|------|------|
| **ISA 规范不足以独立实现**（A1/A2 并行的地基塌了） | M0 验收就是"第三方读规范能写前端"；golden 语料 ≥200 例是可执行判据，不靠文档自评 |
| **ISA 过早冻结锁死优化空间** | M0 只规范化"现状 + 校验规则"，不承诺兼容窗口；发布契约推到 M4（语言特性已稳定后）；核心/扩展分层 + 能力位是逃生舱 |
| A1/B 接口漂移（B 依赖的 Value API 频繁变） | M0 冻结 API + API 变更需跨轨评审 |
| FFI 边界双 GC（Go builtin 的 weak 与 Rust 堆） | 句柄表统一管理；对象生命周期以 Rust 侧为准；FFI 隔离测试 |
| verifier 强度不足而 JIT 已信任它 | verifier 放在 M0（不推迟）；M5 JIT 的每个"省略运行期检查"都要指明它依赖哪条 verifier 规则 |
| C 轨工具链与前端输出格式漂移 | C 使用"编译产物规范"而非引擎内部 API（通过 CLI 子命令集成测试） |
| D 轨 conformance 迁移滞后 | 每轨投入 D 的人力占比固定（≥0.5 人月/里程碑） |
| 并行导致的合并冲突 | 单仓库 worktree 分支 + 逐里程碑合并，禁止长分支 |
| **双实现长期并存的维护税** | M7 退役门禁写死；未满足前 Rust 版以 preview 分发 |

---

## 7. 验收汇总（16 项硬指标）

| # | 指标 | 目标 | 里程碑 |
|---|------|------|--------|
| 1 | ISA 规范可独立实现 | 第三方据规范可写前端 | M0 |
| 2 | golden 字节码语料 | ≥200 例覆盖全指令 | M0 |
| 3 | Go 版 verifier 强化 | "通过即安全"（栈深/try 表/跳转目标） | M0 |
| 4 | golden 语料行为一致 | 100%（aluvm vs Go VM） | M1 |
| 5 | node conformance | ≥8/11（M1，经 Go 前端）→ 11/11（M2） | M1/M2 |
| 6 | node22 conformance | 17/17 | M2 |
| 7 | express 链路 | 通过 | M2 |
| 8 | 双向字节码等价 | 四象限全绿（语义等价） | M2 |
| 9 | test262 子集 | 与 Go 版同分或更高 → **前提 1** | M2 |
| 10 | gcPressure vs node | ≤3×（Go 版 21×） | M3 |
| 11 | arrayPush / fib30 vs node | ≤5× / ≤3× | M3 |
| 12 | 玩具 DSL 前端 | 零改后端跑通 → **前提 2** | M4 |
| 13 | propAccess / closureCall vs node | ≤5× / ≤5× | M5 |
| 14 | jitdiff 迁移 | ≥3 千例零失配 | M5 |
| 15 | build / webbuild | 24/24、13/13 | M6 |
| 16 | 全 conformance + Miri | 9 套 + node22 差分全绿；0 高危 unsafe | M7 |

---

## 8. Go 版退役门禁（与 plan §4.1 同步，缺一不可）

| # | 门禁 | 判据 | 达成里程碑 |
|---|------|------|-----------|
| 1 | 语法完全兼容 | test262 子集与 Go 版同分或更高；9 套 conformance + node22 差分全绿 | M2 + M7 终检 |
| 2 | ISA 契约生效 | `.aluc`/`.alua` 格式发布；玩具 DSL 前端验证通过 | M4 |
| 3 | 性能不退步 | bench 矩阵逐项 ≤ Go 版（gcPressure/closureCall 应显著更优） | M3 + M5 |
| 4 | 生态可用 | `npm i` + express/React SSR/Vue SFC 真实链路通过 | M2 + M6 |
| 5 | 工具链齐备 | `build --compile` / `--target=web` 产物与 Go 版一致 | M6 |
| 6 | 平台覆盖 | Go 版支持的 5 个构建目标全部可用 | M7 |
| 7 | 嵌入 API | `pkg/aluka` 的等价 C ABI / Rust API 可用，现有嵌入方可迁移 | M7 |
| 8 | unsafe 审计 | Miri 零高危；unsafe 白名单逐项有 SAFETY 论证 | M7 |

门禁 1 与 2 即 plan §0.1 的两个硬前提（完全兼容 JS/TS 语法、ISA 契约落地）。
**未全部满足前**：Go 版保持可构建、可发布，Rust 版以 preview 身份并行分发，
不做"两版都是正式版"的承诺。

---

## 9. 附录：轨道 B 内置模块 Rust 化排序（按依赖热度）

第一批（M2 FFI 后直接可用）：fs、url、path、events、util、assert、stream、
buffer、timers、os、string_decoder、querystring、module、process、console
第二批（M3 Rust 化）：crypto、zlib、http、https、net、tls、dns、child_process、
worker_threads、perf_hooks、readline、repl、sqlite
第三批（M5-M6）：v8、inspector、vm、dgram、http2、cluster、wasi、test、
diagnostics_channel、async_hooks、domain、punycode

各模块验收：node22 双跑 diff 零差异 + 专项 conformance（tests/compat/node22 现有用例复用）。