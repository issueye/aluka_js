# aluka Rust 重构计划（功能全景 + 仔细设计）

> 状态：规划稿（v0.3）｜日期：2026-09-03
> 目标读者：重构实施人 / 架构评审
> 依据：本仓库 Go 实现（`aluka_g/`）与 Rust 骨架（`aluka_r/`）、全部 ADR 与性能报告
> 关联决策：`docs/adr/jvm-style-bytecode-architecture.md`（ISA 契约，已采纳）
>
> **仓库布局**：Go 实现在 `aluka_g/`，Rust 实现在 `aluka_r/`，`docs/` 为两者共享。
> 本文位于 `aluka_r/docs/`（Rust 重构专属文档目录）。
>
> **路径约定**：裸写的 `internal/…`、`cmd/…`、`pkg/…`、`tests/…`、`bench/…` 一律
> 相对 `aluka_g/`（Go 侧现状）；`crates/…` 相对 `aluka_r/`；带前缀的 `docs/…`、
> `aluka_g/…`、`aluka_r/…` 相对仓库根。与本文同级的重构文档直接写文件名。

---

## 0. 执行摘要

aluka 是一个纯 Go 实现的、API 行为兼容 Bun/Node.js 的 JS/TS 运行时，核心全部
自研（引擎、模块系统、事件循环、TS 转译、RegExp、GC、包管理器、打包器、GUI、
IPC）。当前 Go 版 17.8 万行 Go 代码（源码 12.6 万 + 测试 5.2 万），
9 个 conformance 套件 + node22 差分对拍。

### 0.1 终局目标：Rust 版取代 Go 版

本计划的终局是**用 Rust 版完全取代 Go 版**（Go 版进入维护期后退役），而非
长期双实现并存。取代有两个硬前提，任一不满足则不得退役 Go 版：

1. **完全兼容 JS/TS 语法**：ECMAScript 全量（Go 版现已达 ES2024）+ TS 注解
   剥离 + JSX/TSX，以 test262 子集与全部 conformance 套件为判据；
2. **字节码契约（ISA）落地**：字节码从"引擎内部缓存"升格为**平台发布契约**，
   使前端与运行时解耦——后续新语法只需在前端产出合规字节码即可无缝接入平台，
   无需改动 aluvm。

### 0.2 为什么是 Rust

技术动机不是"Go 不行"，而是 **Go 的 GC + interface 内存模型对上 JS 引擎的
分配密集负载存在结构性上限**——本仓库用四个实验（ADR：object-arena、
stage2-nanbox-slots、汇编直写、双栈）证明了该结论：Go 无法做 bump 分配的年轻代、
无法做 NaN-box 槽位、无法 unboxed 数值入 interface。Rust 的显式内存模型天然
放开这三条路（arena/盒位/数值直存都可做），目标是 **性能全面对齐 V8 到 2-5x**
而非当前 2-45x。

### 0.3 架构形态：alukac + aluvm（JVM 式分离）

执行链路本来就是 JVM 式的（源码→AST→字节码→VM→JIT 分层）。本次重构把它
**显式拆成两个可独立演进的组件**：

```
                    ┌─ ISA 契约（.aluc / .alua，版本化 + 能力位）──┐
   JS / TS / JSX ─→ │ alukac（前端：词法/语法/降级/字节码优化）      │
   未来新语法   ─→  │                                              │
                    └──────────────────┬───────────────────────────┘
                                       ↓ 字节码
                    ┌──────────────────────────────────────────────┐
                    │ aluvm（后端：加载 + verifier + VM + JIT + GC） │
                    └──────────────────────────────────────────────┘
```

三重价值：
- **新语法零成本接入**（对应 0.1 前提 2）：任何前端只要产出合规字节码即可运行；
- **重构可真并行**：前端与后端只经 ISA 规范耦合，两条轨道独立开发/测试/替换；
- **迁移期风险可控**：Rust 后端可先吃 Go 前端产出的字节码，反之亦然，缺陷定位
  从"整链对拍"细化为"字节码层 + 行为层"两级。

### 0.4 推进方式

**逐子系统替换**（strangler），不是重写。运行时先 Rust、工具链沿用 Go 过渡、
以 conformance 套件作唯一行为仲裁。计划分 **8 阶段（阶段 0-7）约 20 个月**
（含 ISA 规范化与 Go 版退役门禁；若各阶段取工期下限且并行度理想可压到 15 个月，
但不应据此排期）。里程碑级拆解、并行轨道与人力分配见
`rust-reimplementation-devplan.md`。

---

## 1. 项目功能全景（Go 版现状，重构的功能基线）

### 1.1 运行时核心（internal/engine，8.9 万行：源码 5.5 万 + 测试 3.4 万）

**JS 语言引擎（双引擎）**
- AST 解释器（--ast）与字节码 VM（--vm，默认）：共享 lexer/parser/compiler
- 字面量/算术/比较/位运算/逻辑、严格与宽松相等、ToPrimitive 全表
- 隐藏类（Shape 树）+ 内联缓存（IC）：属性读写、方法调用 CallIC、直接映射
  字面量站点缓存
- 值系统：Object/Array/Function/String/Number（slab 装箱）/BigInt/Symbol/
  Boolean/Null/Undefined/错误对象/Date/RegExp/WeakMap/TypedArray/ArrayBuffer/
  DataView/Proxy
- 属性描述符完整语义：writable/enumerable/configurable、accessor、seal/freeze/
  preventExtensions、Array exotic（holes/length/species）、Proxy invariants
- ES 语法：ES5 全部、ES2015（class/箭头/解构/模板串/模块/生成器/迭代器
  /Symbol/Map/Set/WeakMap/WeakSet/Proxy/Reflect/Promise/async）、ES2017-24
  （async/await、TLA、for await、可选链、空值合并、BigInt、动态 import、
  import attributes、Promise.withResolvers、Array.fromAsync、Object.groupBy、
  装饰器 Stage 3/TS）

**执行架构**
- Tier 0：AST 解释器 + 字节码 VM（编译一次、9 个优化 pass：常量折叠/不可达删除
  /融合/跳转穿透）
- Tier 1：Quick JIT（类型化 IR，可执行 Go）
- Tier 2：Native JIT（amd64 机器码，W^X/崩溃隔离/safepoint/OSR）；property PIC、
  arrayIndex/arrayPush/arrayBatch/closureIncrement/upvalue 等专用快路径
- GC：Go GC 承载物理回收（weak.Pointer 注册表 + 周期清扫 + 显式标记）；
  数字 slab 分配
- 字节码磁盘缓存（.aluka-cache，FormatVersion 校验）
- V8 风格栈迹（错误对象 name/message/stack）

**RegExp**
- RE2 翻译快路径 + 自研回溯 fallback（lookaround/backref/类子集）
- legacy/u 模式 UTF-16 索引语义、lastIndex、replace/split 偏移、回溯预算

**TS/JSX**
- 源码级转译（类型注解剥离在 parser 层）、TSX/JSX 编译期 lowering

### 1.2 模块系统（internal/runtime/module，6.5K 行）
- ESM 全语法 + CJS（require/module.exports）+ Node 解析算法
- package.json 深度：exports 条件映射、imports、main、module、type、browser/
  node 实例级条件隔离
- 循环依赖、TLA、.ts 相对导入、import attributes、路径别名（paths/baseUrl）
- 字节码磁盘缓存

### 1.3 Node.js 内置库（internal/builtin，3.1 万行，58 个 `RegisterBuiltin` 模块）
fs（sync/promises/cp）、path（posix/win32）、os、url、util（types）、events、
assert（strict）、constants、crypto（含 WebCrypto）、stream（web/promises/
consumers）、string_decoder、http、https、net、tls、dns（promises）、zlib（zstd）、
perf_hooks、timers（promises）、v8（writeHeapSnapshot）、vm、inspector
（CDP 会话）、dgram、http2、cluster、trace_events、readline（promises）、repl、
child_process、worker_threads、module、buffer、tty、sqlite（DatabaseSync）、
domain、punycode、wasi、process、console、test（node:test + reporters）、
async_hooks、diagnostics_channel

### 1.4 Web API & 全局（internal/runtime/globals，1.9 万行，12 子包）
- console、navigator、BroadcastChannel、MessageChannel、AbortController
- fetch 全家（Request/Response/Headers/FormData）、WebSocket、URL/URLPattern
- Streams（可读/可写/转换）、CompressionStream（gzip/deflate/deflate-raw）、
  Blob/File、structuredClone
- WebCrypto `crypto.subtle`、TextEncoder/Decoder、atob/btoa
- timers/performance、Intl 全家（DateTimeFormat/NumberFormat/RelativeTimeFormat/
  ListFormat/PluralRules/Collator/Segmenter）
- Aluka（Bun 兼容）：serve/file/write/$/env/sleep/hash/password/deflate/
  inflate/spawn/ipc、SQL（sqlite+postgres）、Redis、S3（自研 SigV4）、shell、
  Bun.peek/deepEquals

### 1.5 工具链（non-runtime，2.2 万行）
- 包管理器（internal/pkgmanager，1.6K 行）：registry 客户端、semver、依赖树
  + hoisting、lockfile、workspace、.npmrc
- 打包器（internal/bundler，1.3 万行）：build --compile（单文件产物+payload 字节码
  +manifest+footer）、build --target=web（graph/shake/minify/emit/chunk/sourcemap/
  ESM-CJS-UMD/watch/dev）、Vue SFC（subset + official compiler-sfc 双后端）
- GUI（internal/gui，6.9K 行）：Windows WebView2、macOS WKWebView（无 CGO）
- IPC（internal/ipc，0.7K 行）：AIP 协议 16B 帧头、管道/UDS/TCP、多路复用、
  PubSub
- project/web 工作台、monitor 指标、pkg/aluka 嵌入 API

### 1.6 CLI（cmd/aluka，5.3K 行）
run/编译产物检测、repl、test、build/install/add/remove/update、--watch/--dev、
--profile（pprof CPU+heap）、--monitor、--ic-stats/--jit*/--max-memory、
--target=web/--vue-compiler

### 1.7 质量基座
- conformance 9 套：node 11/11、node22 17/17、build 24/24、webbuild 13/13、
  vue-sfc 1/1、npm、install、express、test262；另有 tests/compat/node22 差分对拍
- jitdiff 生成式差分（三 tier 零失配）+ 5 个 fuzz target
- bench 跨引擎对比（node/aluka 交替，方法学见 tests/benchmark/）
- `docs/` 50 份顶层计划+报告（另有 `docs/adr/` 10 份 ADR；Rust 重构专属文档在 `aluka_r/docs/`）

---

## 2. 为什么用 Rust 重构（Go 版实测结论 → Rust 解决）

| Go 版约束（已实验证实） | 权威证据 | Rust 版解决 |
|---|---|---|
| 对象无法 arena/bump 年轻代分配（存活 pin 整块，级联保活） | ADR object-arena（RSS 放大 22-71x） | 自研分配器：bump 年轻代 + 标记-清除或 RC，`Box`/`Vec` 或手动 arena，对象地址可搬移（copying GC 可选） |
| 槽位无法 NaN-box（[]uint64 内指针 GC 不可见 → 悬垂） | ADR stage2-nanbox-slots（weak 实证回收） | Value 设为 `u64`（NaN-boxing 或 tagged union），引用分代由引擎自管；无 Go GC 干涉 |
| 数值无法 unboxed 入 interface（convT64 8B/次；unsafe 直写消费端 fault） | ADR 汇编/unsafe 节 + 双栈实验 | `enum Value { Num(f64), ... }` 栈内/槽内直接内联数字；Rust 无"接口 data 字"概念 |
| JIT 只到 amd64 单层、Quick 跨平台但慢 | perf-report-v7 等 | Cranelift 或手写后端可做多目标（x86-64/ARM64）+ 更成熟的 regalloc（参考 V8/SpiderMonkey） |
| 无逃生分析 -> 21x gcPressure | perf-report | Rust 可做标量替换/逃逸分析（对象内联进栈），对齐 V8 |

**注意反向风险**：Rust 无 GC 默认 → 循环引用/自引用（JS 对象图任意引用）需要
设计选择（见 §6.3）；unsafe 面必须严格收口。

---

## 3. 目标架构（alukac / aluvm 两组件 + ISA 契约）

### 3.1 组件划分

字节码是两个组件之间的**唯一接口**。crate 按归属分组，跨组依赖只允许经
`aluka-bytecode`（ISA 定义与序列化）：

```
crates/
  # ── 共享契约（唯一跨组依赖）───────────────────────────────
  aluka-bytecode/      ISA：指令集 + 元数据 + 序列化 + verifier + 优化 pass

  # ── alukac 前端（源码 → 字节码）──────────────────────────
  aluka-parser/        lexer/parser/AST（JS + TS 注解 + JSX）
  aluka-compiler/      AST→字节码 + TS 剥离 + JSX lowering
  aluka-cc/            alukac 二进制：编译、产出 .aluc/.alua

  # ── aluvm 后端（字节码 → 执行）───────────────────────────
  aluka-core/          值系统 + 堆 + Shape（Value=u64、对象盒）
  aluka-gc/            GC（阶段 3 选型落地）
  aluka-vm/            Tier 0 解释器
  aluka-jit/           Quick IR → Cranelift/自研后端（阶段 5）
  aluka-regex/         RE2 风格 + 回溯引擎
  aluka-module/        ESM/CJS 模块系统 + resolver（实例级条件）
  aluka-builtins/      node:* 内置（迁移期经 FFI 复用 Go 实现）
  aluka-webapi/        Web API + Aluka（Bun 兼容）全局
  aluka-runtime/       运行时装配门面
  aluka-vm-bin/        aluvm 二进制：加载 + 校验 + 执行

  # ── 工具链（两组件之外）──────────────────────────────────
  aluka-pkg/           包管理器
  aluka-bundler/       打包器（graph/shake/minify/emit）
  aluka-cli/           aluka 便利壳（等价 `java` 直接跑源码的糖）
  ffi/                 cabi 绑定：Go 宿主、GUI WebView2 等
```

对应 Go 版：engine 的 lexer/parser/ast/compiler → 前端；engine 的
bytecode → 共享契约；engine 的 interpreter/jit/regex/value/shape/gc +
runtime/* + builtin → 后端；pkgmanager/bundler → 工具链。

### 3.2 为什么这样分组

- **`aluka-bytecode` 是双向依赖的唯一出口**：前端只写它、后端只读它。任何
  "前端直接调用后端类型"的诱惑都必须拒绝，否则 ISA 契约退化为空文。
- **`aluka-core` 归后端**：值表示、堆布局、Shape 是执行期概念；前端只产出
  常量池条目（数字/字符串/BigInt 的字面表示），不接触 `Value`。
- **verifier 在 `aluka-bytecode` 而非后端**：它是契约的一部分（"什么样的
  字节码是合法的"），前端可用它自检产物，后端在加载期强制执行。

### 3.3 新语法接入的成本模型

前提 0.1-2 的具体含义：新增语法（无论是 ECMAScript 新提案、TS 新语法，还是
自定义 DSL）的接入成本取决于它需要什么：

| 情形 | 需改动 | 举例 |
|------|--------|------|
| **纯语法糖**：可降级为现有指令 | 仅前端 | 可选链、空值合并、装饰器、JSX、`??=` |
| **需新语义原语**：现有指令无法表达 | 前端 + ISA 扩展指令 + 后端 | 生成器（需挂起/恢复）、`await`、Proxy trap |
| **需新对象模型** | 前端 + ISA + 后端 + GC | WeakRef、Records/Tuples（若采纳） |

绝大多数新语法落在第一类——这是"无缝接入"成立的前提，也是把降级
（lowering）职责放在前端的理由。第二、三类走 ISA 扩展指令 + 能力位协商
（见 `docs/adr/jvm-style-bytecode-architecture.md` §4.2），旧 aluvm 遇到
未知能力位应**拒绝加载**而非误执行。

---

## 4. 阶段路线（阶段 0-7，约 20 个月，每阶段有验收门）

各阶段给出工期区间；`devplan` 取区间上限做点估计以便排期（M0 2 / M1 3 / M2 4 /
M3 3 / M4 1.5 / M5 3.5 / M6 2.5 / M7 1.5 月）。分属不同轨道的相邻阶段允许
首尾重叠。

### 阶段 0：价值锚定 + ISA 规范化（1.5-2 个月）
- 冻结 conformance 套件与 bench 基线
- 选定 Rust GC 策略并做微原型（§6.3）
- **把 `docs/bytecode-spec.md` 从"实现说明"提升为"规范"**：补齐校验规则、
  一致性用例、扩展指令的能力位协商约定
- **Go 版 verifier 强化到"通过即安全"**（跨块栈深合流、try 表结构、
  跳转目标合法性）——这一步对 Go 版本身也是净安全收益
- **建立 golden 字节码语料**：Go 前端产出，作为两侧共同的输入基线
- 验收：ISA 规范可据以独立实现前后端；golden 语料 ≥200 例覆盖全指令

### 阶段 1：aluvm 骨架 + 解释器（2-3 个月）
- Value（表示由阶段 0 定案）+ 对象盒 + Shape/IC
- 字节码加载 + verifier + Tier 0 全指令
- **关键并行收益**：输入用 Go 前端产出的 `.aluc`，**不必等 Rust 前端**
- 里程碑：`aluvm hello.aluc` 跑通；fib30 达 Go 版 2x
- 验收：golden 语料 100% 行为一致；node conformance ≥8/11（经 Go 前端）

### 阶段 2：alukac 前端 + 全语言（3-4 个月，与阶段 3 部分并行）
- 前端：lexer/parser/AST + TS 剥离 + JSX lowering + 字节码生成
- 后端：ES2024 全量语义、RegExp、BigInt、Proxy/Reflect、异步/生成器/TLA
- 双向对拍：Rust 前端产物喂 Go VM、Go 前端产物喂 aluvm
- node:* 模块（先 FFI 到 Go builtin，逐步 Rust 化）
- 验收（**对应终局前提 1**）：node22 17/17 + webapi 套件绿 + test262 子集
  与 Go 版同分；Rust 前端与 Go 前端对同一源码产出**语义等价**字节码

### 阶段 3：GC 真章（2-3 个月）
- 选择：A) 分代标记-清除（bump 年轻代 + 卡表）B) 引用计数 + 周期回收
  C) RC+分代混合
- 数字内联、数组 packed elements（f64 数组直存）
- 里程碑：gcPressure 对齐 node ≤3x（当前 21x）
- 验收：test262 子集 + jitdiff 等价物零失配

### 阶段 4：ISA 发布契约（1-1.5 个月）
- 定 `.aluc`/`.alua` 二进制格式（含调试信息段与剥离选项）
- 拆 `alukac`/`aluvm` 二进制；`aluka` 退化为便利壳
- 决定 `eval`/`new Function` 策略（内嵌编译器 or 受限模式）与
  `Function.prototype.toString` 合规性（源片段 or 降级）
- 承诺兼容窗口与核心 ISA 版本递增权限
- 验收（**对应终局前提 2**）：第三方前端可据规范产出可执行字节码
  （用一个玩具 DSL 前端验证"新语法无缝接入"）

### 阶段 5：JIT（3-4 个月）
- Quick IR → Cranelift 后端（或先自研 amd64）
- PIC/shape guard/去优化恢复（对应现有 jitdiff 全用例）
- 里程碑：propAccess/closureCall ≤ node 5x
- 验收：新 fuzz + 三 tier 零失配

### 阶段 6：工具链迁移（2-3 个月，与阶段 5 并行）
- pkgmanager/bundler/Vue SFC/CLI 逐个 Rust 化；GUI 经 ffi 桥接

### 阶段 7：Go 版退役（1-2 个月）
- AIP/IPC、监控、堆快照、inspector CDP、测试运行器补齐
- **退役门禁**（全部满足方可执行，见 §4.1）
- Go 版打最后一个维护 tag，仓库主线切到 Rust 实现

### 4.1 Go 版退役门禁（硬性，缺一不可）

| # | 门禁 | 判据 |
|---|------|------|
| 1 | 语法完全兼容 | test262 子集与 Go 版同分或更高；9 套 conformance + node22 差分全绿 |
| 2 | ISA 契约生效 | `.aluc`/`.alua` 格式发布；玩具 DSL 前端验证通过 |
| 3 | 性能不退步 | bench 矩阵逐项 ≤ Go 版（gcPressure/closureCall 应显著更优） |
| 4 | 生态可用 | `npm i` + express/React SSR/Vue SFC 真实链路通过 |
| 5 | 工具链齐备 | build --compile / --target=web 产物与 Go 版一致 |
| 6 | 平台覆盖 | Go 版支持的 5 个构建目标全部可用 |
| 7 | 嵌入 API | `pkg/aluka` 的等价 C ABI/Rust API 可用，现有嵌入方可迁移 |
| 8 | unsafe 审计 | Miri 零高危；unsafe 白名单逐项有 SAFETY 论证 |

**未满足前**：Go 版保持可构建、可发布，Rust 版以 preview 身份并行分发。

---

## 5. 逐子系统迁移核对表（来源：§1 功能全景）

每个子系统三态：`Go 沿用` / `FFI 过渡` / `Rust 重写`，附迁移验收标准。
"组件"列标明它归 alukac 前端（C）、aluvm 后端（V）、共享契约（S）还是
工具链（T）——这决定了它属于哪条并行轨道。

| 子系统 | 组件 | 状态 | 验收 |
|---|---|---|---|
| ISA 规范 + verifier + golden 语料 | S | Go 先强化→Rust 重写 | 规范可据以独立实现；语料 ≥200 例 |
| lexer/parser | C | Rust 重写 | test262 语法子集 |
| TS 剥离 / JSX lowering | C | Rust 重写 | TSX 样例 + vue-sfc |
| 字节码生成 + 优化 pass | C | Rust 重写 | optimize-equivalence 对拍 |
| 值系统 / Shape / 堆 | V | Rust 重写 | 单元 + jitdiff |
| GC | V | Rust 重写 | gcPressure ≤3x node |
| VM Tier0 | V | Rust 重写 | golden 语料 100% + node22 17/17 |
| JIT | V | Rust 重写 | jitdiff 零失配 |
| RegExp | V | Rust 重写 | regex corpus + Node oracle |
| ESM/CJS 模块 | V | Rust 重写 | npm/import conformance |
| node:* 58 个 | V | FFI 过渡→Rust | builtin conformance |
| Web API | V | Rust 重写 | webbuild/vue-sfc |
| Aluka API | V | Rust 重写 | express/es 套件 |
| pkgmanager | T | Go 沿用→重写 | install/npm |
| bundler | T | Go 沿用→重写 | build/webbuild |
| GUI/IPC | T | FFI 桥接 | gui smoke |
| CLI | T | 过渡期 Go 壳 | cli 冒烟 |

---

## 6. 关键设计决策（需阶段 0 前敲定）

### 6.1 Value 表示
- 首选：`u64` NaN-box（lower 51 位 NaN 编码 pointer/f64/小整数/特殊值）
- 备选：`enum Value{t}` 也可，但比对/复制开销更大；跟随 Go ADR 结论定
- 关联：数字直存（f64 内联）、字符串 rope、BigInt 堆盒
- **注意**：Value 是后端内部表示，**不进 ISA**。字节码只见常量池条目与
  指令操作数，因此该决策可以在 ISA 冻结后继续演进

### 6.2 对象布局
- Shape 树（迁移 Go 现有设计）+ 内联属性（specified inline slots）
- Packed double elements（`Vec<f64>` 数组）、hole 位图
- 隐藏类不需要 Go 版"小槽位/PIC 上限"等变通（Rust 无 GC 可见性约束）

### 6.3 GC 策略（最高风险决策）
- 建议：**分代 + 卡表标记-清除**（年轻代 bump、老年代 mark-sweep），根集由
  VM/栈显式提供
- 必须解决：跨代引用（写屏障）、栈扫描（调用栈是 Rust 物理栈——用 shadow stack
  或栈顶 root 列表）、`extern` 边界（FFI/内建持有对象）
- 工程兜底：RC + cycle collector 更易正确但性能上限低；最终取舍看阶段 3 原型

### 6.4 unsafe 收口策略
- unsafe 仅限：GC 遍历/分配器/FFI/JIT 机器码生成
- 编译器 `#![forbid(unsafe_code)]` 于业务 crate + 特批白名单
- 每 unsafe 点要求注释（SAFETY）+ 审查清单
- **与 verifier 的关系**：JIT 后端会信任 verifier 的结论（假定字节码合法）
  来省略运行期检查。因此 verifier 的强度直接决定 unsafe 代码的安全性——
  这是把 verifier 放进阶段 0（而非推迟）的根本原因

### 6.5 兼容性仲裁
- conformance 套件（现 25 个）作为唯一行为标准；diff 方法学沿用
  （交替执行、时钟分辨率、循环消除修正）
- **迁移期新增字节码层仲裁**：Rust 前端与 Go 前端对同一源码的产出须语义
  等价（允许指令序列不同，但执行结果与可观察副作用必须一致）

### 6.6 ISA 演进权限与兼容窗口（阶段 4 定案）
- 核心 ISA：承诺跨小版本兼容；递增需架构评审
- 扩展指令：header 能力位声明，旧 aluvm 遇未知位**拒绝加载**（不得误执行）
- JIT 内部 IR 不属于 ISA，可自由变更（现有三层 tier 的 IR 已是内部的）

---

## 7. 风险登记与应对（Top 12）

1. GC 正确性（对象任意引用图）→ 阶段 0 微原型 + 大量 fuzz/差分
2. 栈根集/写屏障性能 → shadow stack 微基准提前做
3. JIT 后端工期 → 先 Cranelift（风险低）再自研
4. unsafe 内存事故 → 白名单 + Miri 定期跑；且以强 verifier 为前提（§6.4）
5. 内置库 58 个模块迁移量 → 先 FFI 过渡保证行为一致，再逐模块 Rust 化
6. FFI 边界内存泄漏/双 GC 交互 → 边界对象用专有句柄表（handle table）
7. 并发模型（worker_threads/async）→ 复用 Go 事件循环语义，Rust async 或
   线程池决策需提前
8. 工具链（pkgmanager/bundler）与引擎解耦 → 阶段 6 独立推进，不阻塞引擎
9. 团队 Rust 熟练度 → 先做 parser/bytecode（低风险练手）
10. 规格漂移（ECMA-262 新特性）→ conformance + test262 持续集成
11. **ISA 过早冻结锁死优化空间** → 阶段 0 只规范化"现状 + 校验规则"，
    不承诺兼容窗口；发布契约推到阶段 4（语言特性已稳定后）。核心/扩展
    分层 + 能力位协商是逃生舱
12. **双实现长期并存的维护税** → 退役门禁（§4.1）写死；未满足前 Rust 版
    以 preview 分发，不做"两版都是正式版"的承诺

---

## 8. 验收与里程碑汇总

阶段 0-7 与 devplan 的 M0-M7 一一对应；下表给出累计时间点，任务级拆解与
并行轨道归属见 `rust-reimplementation-devplan.md` §2/§4。

| 里程碑 | 阶段 | 轨道 | 时间 | 验收 |
|---|---|---|---|---|
| M0 ISA 规范化 + GC 选型 | 0 | F(+A1/E 原型) | +2 月 | 规范可独立实现；golden ≥200 例；微基准报告 |
| M1 aluvm 跑 golden | 1 | A1(+A2 起步) | +5 月 | golden 100% 一致；node ≥8/11（经 Go 前端） |
| M2 alukac 前端 + 全语言 | 2 | A1+A2+B | +9 月 | **前提 1**：node22 17/17、test262 同分、双向字节码等价 |
| M3 GC/数组 | 3 | A1+B | +12 月 | gcPressure ≤3x node |
| M4 ISA 发布契约 | 4 | F | +13.5 月 | **前提 2**：`.aluc`/`.alua` 发布；玩具 DSL 前端验证 |
| M5 JIT | 5 | E | +17 月 | jitdiff 零失配、call ≤5x |
| M6 工具链 | 6 | C | +19 月 | build/webbuild 全绿 |
| M7 Go 版退役 | 7 | 全轨签核 | +20 月 | §4.1 八项门禁全过 |

---

## 9. 附录

- Go 版功能清单出处：README.md 核心能力一览（§1）、`docs/` 50 份顶层文档 + `docs/adr/` 10 份 ADR、tests/conformance
- 性能基线：docs/performance-report-v7.md
- 内存/GC 探索结论：docs/adr/object-arena-rejected.md、stage2-nanbox-slots-rejected.md、
  typed-value-stack-plan.md §9/§10
- JIT 现状：docs/jit-performance-optimization-plan.md、jit-coverage-matrix.md