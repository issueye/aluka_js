# Aluka · 全局总 TODO 与路线图规划 (Node.js 22 LTS 目标)

> **核心定位**：纯 Rust 实现的高性能、现代化 JavaScript 与 TypeScript 运行时。  
> **唯一权威基准**：官方 **Node.js 22 (LTS v22.23.1+)** 行为与 ECMAScript / test262 标准规范。  
> **架构契约原则**：前端编译器（`alukac`）与执行后端（`aluvm`）以标准 ISA 字节码严格解耦开发；后续达成 Node.js 全面兼容后考虑程序合并，**内部执行与解耦流程保持不变**。  
> **每日待办落地**：每日工作与真实证据记录在 `.work/TODO/<YYYYMMDD>/README.md`，模板见 [TEMPLATE.md](./TEMPLATE.md)。

---

## 0. 证据规则（不满足即不许勾选结项）

「达成证据」是任何人均可在任何合规环境下独立复现的客观凭证，分为以下三类之一：

| 证据类型 | 规范形式 | 典型示例 |
|---|---|---|
| **命令证据** | 执行命令 + 关键无歧义输出摘要（杜绝无意义海量日志刷屏） | `cargo test -p aluka-bytecode` → `17 passed; 0 failed` |
| **产物证据** | 真实入库或生成的产物绝对/相对路径 + 校验方式（行数 / SHA256） | `tests/conformance/node22/cases/01.cjs` + 差分一致校验 |
| **提交证据** | 确切的 Git Commit Hash + 规范 Commit 信息（简述变更范畴） | `a1b2c3d` `feat: 支持 Proxy get/set 核心 traps` |

### 四条硬性纪律
1. **严禁主观臆断**：严禁以"应该没问题"、"看起来通过了"等主观推断作为结项依据；测试失败必须明确记录失败原因与卡点；
2. **差分对拍原则**：以官方 Node.js 22 LTS 本地运行时（`node <script>`）作为唯一 oracle 对拍源；
3. **性能测试方法学**：性能测试必须遵守「交替执行 + 冷却时间 + min-of-N（取 N 次运行最小值）」的严密方法学，排除系统调度抖动；
4. **正确性绝对优先**：性能与规范正确性发生冲突时，规范正确性拥有绝对优先权。

---

## 1. 架构组件分工与演进路线

```
当前阶段（独立开发与验证）：
                 ┌─ ISA 字节码契约（aluka-bytecode）────────┐
      JS / TS ─→ │ alukac（前端：词法/语法/AST/TS剥离/优化）   │
                 └──────────────────┬───────────────────────┘
                                    ↓ 强类型标准字节码 (.bc / .aluc)
                 ┌──────────────────────────────────────────┐
                 │ aluvm（后端：Verifier + VM 解释循环 + 内置库）│
                 └──────────────────────────────────────────┘

终局形态（达成 Node.js 22 全面兼容后）：
                 ┌─ 统一单二进制入口（aluka run / aluka build）──────┐
                 │ 内部流程不变：源码 → 编译/静态校验 → VM 解释执行 │
                 └──────────────────────────────────────────────────┘
```

为了保持前后端组件解耦与高效推进，设立前后端专属任务跟踪视图：
- 🚀 **[前端编译器（alukac）专属任务清单](./frontend/README.md)**：负责语法解析、AST 构建、TS 类型剥离、字节码生成与前端优化；
- ⚡ **[后端虚拟机（aluvm）专属任务清单](./backend/README.md)**：负责字节码校验、指令解释、堆与 GC、内联缓存与 Node.js 22 标准内置库。

---

## 2. 里程碑总览 (M1 ~ M7)

| 里程碑 | 核心目标 | 关键验收指标 | 状态 |
|---|---|---|:---:|
| **M1** | **ECMAScript 核心规范收口** | Proxy/Reflect（13 traps）、RegExp Lookbehind/命名组、ES2024 不可变数组、eval / new Function 动态求值、test262 扩容 ≥100 例 | `[x]` |
| **M2** | **模块系统与真实生态承载** | `package.json` `exports`/`imports` 条件映射规范、Top-Level Await、**Express 100% 跑通真实依赖树与 Web 服务** | `[x]` |
| **M3** | **核心内置模块生产级闭环** | Stream 规范背压状态机、纯 Rust TLS 1.3 握手、HTTP 1.1 生产级长连接与连接池（http2 表面）、异步 DNS 递归查询 | `[x]`（M3.1–M3.4 全部达成；M3.4b resolve 家族真实递归查询已闭环，见 §M3.4，20260909 结项登记） |
| **M4** | **现代 Web API 标准对齐** | 规范级 Fetch API、Web Streams 与 Node Streams 原生互通、`AbortController` 全系统级联动中断 | `[x]` |
| **M5** | **多线程并发与进阶能力** | `worker_threads` 真实跨物理线程 Worker、`cluster` 进程池、`node:sqlite` 原生数据库支持 | `[~]` → **M5 全部子项达成（20260912 结项）**：M5.1 ✅ / M5.2 ✅（RR 调度按架构级偏离结项）/ M5.3 ✅ / M5.4 ✅（LCOV + 真 Transform 报告器闭环）；登记偏离见 §M5 各行与 20260912/README.md |
| **M6** | **生产级 GC 与高性能引擎** | 分代标记-清除 GC 正式合入主流程、8 字节 NaN-boxing 切换、多态内联缓存（PIC）与 JIT 全指令流扩容 | `[ ]` |
| **M7** | **终局合并与全面验收** | `alukac` 与 `aluvm` 合并为统一 `aluka` 单二进制（流程不变）、Node.js 22 官方套件 ≥1000 例全绿通过 | `[ ]` |

---

## 3. 里程碑细分待办清单

### M1 · ECMAScript 核心规范收口 (ES2024 / test262)

> **2026-09-09 复评确认达成**（证据与结论见 `20260907/README.md` 与
> `20260909/README.md` §11；结项提交 `30e11ff`、`127bf71`）：test262 语料
> **154/154 实跑通过**（M1.1 proxy 45 例 / M1.2 regexp 12 例 / M1.3 es2024
> 10 例 / M1.4 typedarray 15 例 / M1.6 eval 52 例 + array/negative），
> `cargo test -p aluka-regex` 15 passed、`aluka-vm --lib` 146 passed。
> 已知降级（Proxy invariant 平凡满足 / direct-eval var 语义 / SAB 进程内
> 共享 / 原型方法属性面按需合成）已在结项轮登记在案。

- [x] **M1.1 Proxy & Reflect 反射代理子系统**
  - 实现 `Proxy` 构造器与 13 种核心 Traps 拦截器（`get`, `set`, `has`, `deleteProperty`, `apply`, `construct`, `getPrototypeOf`, `setPrototypeOf`, `isExtensible`, `preventExtensions`, `getOwnPropertyDescriptor`, `defineProperty`, `ownKeys`）；
  - 实现全局 `Reflect` 对象的 13 个静态规范方法；
  - 验收：通过 Proxy/Reflect 专属测试套件（≥40 用例）。✅ 45 例全过（m1-proxy-001..045，覆盖 13 traps + revocable + Reflect 13 方法）
- [x] **M1.2 RegExp 引擎进阶语法**
  - 正向后行断言 `(?<=...)` 与负向后行断言 `(?<!...)`；
  - 命名捕获组反向引用 `\k<name>` 与替换语法 `$<name>`；
  - 验收：RegExp 语法矩阵 100% 与 Node.js 22 LTS 差分对齐。✅ aluka-regex 15 passed（含 lookbehind/backref/词边界）+ test262 regexp 12 例
- [x] **M1.3 ES2022~ES2024 新增标准方法**
  - `Promise.withResolvers` 规范实现；
  - 数组不可变变更方法：`toSorted()`, `toReversed()`, `toSpliced()`, `with()`；
  - 分组方法：`Object.groupBy()`, `Map.groupBy()`；
  - 字符串 Well-Formed 校验与转换：`isWellFormed()`, `toWellFormed()`；
  - 验收：单测与 Node.js 22 对拍一致。✅ test262 m1-es2024-001..010 全过
- [x] **M1.4 类型化数组 (TypedArray) 规范体系**
  - 完备的 `ArrayBuffer`、`SharedArrayBuffer`、`DataView` 内存操作；
  - 11 种 TypedArray 构造函数与完整原型链继承关系；
  - 验收：TypedArray 二进制存取与字节序测试全绿。✅ test262 m1-typedarray-001..015 全过（11 构造器/DataView LE·BE/Uint8Clamped/subarray）
- [x] **M1.5 test262 官方测试集扩容**
  - 接入官方 test262 标准测试 runner，测试集规模从 8 例扩容至 ≥100 例；
  - 验收：`cargo test -p aluka-cli --test test262_subset_test` 100% 通过。✅ 154/154 passed（20260909 复测）
- [x] **M1.6 动态代码求值子系统 (`eval` & `new Function`)**
  - **直接调用 `eval(code)` (Direct Eval)**：实现调用栈帧与局部词法作用域穿透，支持在当前局部环境内即时求值；
  - **间接调用 `eval(code)` (Indirect Eval)**：严格限制在全局作用域下求值，隔离局部调用帧；
  - **动态函数构造器 `new Function(...args, body)` / `Function(...)`**：实现形参与函数体字符串拼接解析、全局作用域函数模板动态生成；
  - **动态字节码 Verifier 安全门禁**：动态编译产出的字节码必须 100% 经由 `aluka-bytecode::verifier` 静态安全校验，杜绝非法跳转与栈溢出；
  - 验收：通过 eval 与 Function 专项测试套件（≥50 用例），与 Node.js 22 LTS 差分对拍 100% 一致。✅ test262 m1-eval-001..052 共 52 例全过
- [~] **M1.7 块内函数声明的绑定与提升**（20260911 发现并部分修复）
  - **背景（引擎级缺口）**：`codegen.rs` 对 `Stmt::Function` 是空实现（只保证栈平衡），
    而提升收集只遍历**直接子语句**——`if`/`for`/普通块内的函数声明既不绑定名字也不可用，
    `typeof f` 恒为 `undefined`、调用即抛 `TypeError`。影响面：任何把辅助函数声明写在
    块内的真实代码（本轮 M5.2 断连探针首版即因此崩溃）。
  - ✅ **已修复（20260911，`cc0b922`）**：新增递归收集
    `collect_scope_functions`（进入 `Block`/`if`/`while`/`do-while`/`for`/`for-in`/
    `for-of`/`try`/`switch`/`export`；不进入嵌套函数体、不进入表达式），模块顶层与
    函数体的提升收集改用它，并补「收集到的函数名 `ensure_slot`」预注册（保证
    `ParentScopeInfo` 快照与上值捕获识别可见）。**块内可调用（含声明之前）、块外可访问、
    函数体内块、上值捕获**均与 Node v22.23.1 逐字节一致；门禁内回归保护用例
    `tests/conformance/node22/cases/gen/gen-block-fn-decl-0002.cjs`。
  - ⚠️ **余差异（隔离登记，未修）**：
    ① **块执行前**引用块内函数名为 `function`（Node 为 `undefined`；方向更宽松、不崩溃）；
    ② **块内函数捕获同块 `let`/`const`** 时读到 `undefined`（功能缺口）——根因是提升函数
    的闭包在函数入口创建（捕获函数级预注册槽），而块级 `let`/`const` 在 `codegen.rs`
    的 `block_depth > 0` 分支总是分配块级新槽。近似方案（块级 `let`/`const` 复用函数级
    槽）已尝试并**回退**：会破坏块级遮蔽（`(() => { let x = 10; { let x = 20; } return x })()`
    实测 Node=`10` / aluka=`20`）。**精确修法**：把绑定动作下移到**块入口**（块内函数
    模板编译期预编译 + 随 `CompiledUnit` 传递 + `Stmt::Block` 分支内 `MakeClosure`）。
  - 证据与完整记录：[20260911/README.md §12.2/§13](./20260911/README.md)；隔离用例
    `gen/deviations/gen-block-fn-decl-000{1,2}.cjs`。

---

### M2 · 模块系统与真实生态承载 (Express 落地)
- [x] **M2.1 现代 `package.json` 解析规范**
  - 支持 `exports` 字段多条件映射（`import`, `require`, `node`, `default` 等条件分支）；
  - 支持 `imports` 内部子路径别名解析（`#internal/utils`）；
  - 支持 `type: "module"` 作用域自动判定与扩展名补全策略；
  - 验收：现代 npm 模块加载用例 100% 通过。
- [x] **M2.2 Top-Level Await (TLA) 规范支持**
  - 编译器支持 AST 顶层 `await` 语法检测与阶段标记；
  - 模块加载器基于 DAG 拓扑排序实现异步模块加载与微任务推进；
  - 验收：TLA 模块加载依赖测试全绿。
- [x] **M2.3 ESM 与 CJS 深度互操作**
  - 实现 `createRequire(import.meta.url)` 动态创建 CJS require 函数；
  - 规范级 `import.meta` 对象（`url`, `filename`, `dirname`, `resolve`）；
  - 支持 ESM 默认导入 CommonJS 模块时的导出属性自动推断；
  - 验收：双模块混合调用集成测试全绿。
- [x] **M2.4 真实第三方生态里程碑：Express 100% 跑通**
  - 排除 http-errors 等前置依赖加载障碍，实现 Express 依赖树完整无误加载；
  - 跑通 6 大核心场景测试：`GET /`、`GET /echo/:word`、`POST /json`、并发压力场景、自定义 Content-Type 与进程优雅退出；
  - 验收：固化 `express_e2e_test.rs` 集成测试，对齐 Node 22 输出。

---

### M3 · 核心内置模块生产级闭环 (I/O & 生产网络)

> **2026-09-09 结项登记（M3.2/M3.4 双闭环）**（证据见 `20260909/README.md` §12/§13）：
> M3.2 TLS 接线闭环（提交 `fb97628`）：VM `https` server/client 双向真实 rustls
> 握手、事件泵 `Stage::Handshaking` 调度推进，Node 22 对拍全绿。
> M3.4b resolve 家族真实递归查询闭环（见 §M3.4 达成证据）：hickory-proto 报文
> 编解码 + 后台 std::thread + crossbeam-channel 桥，callback/promises 双面
> 与 Node 22 实时对拍逐字一致。全套回归通过，fmt/clippy 零告警。总览 `[x]`。
>
> **2026-09-09 M3 最终验收快照**（证据见 `20260909/README.md` §14）：
> 最终提交点（`4bd91d8`/`245acae`）后全量复跑 M3 验收集——stream 4/4 +
> http 10/10 + net 8/8 + https_tls_loopback 1/1 + aluka-vm lib 151/151，
> **合计 174 passed / 0 failed**；M3.1–M3.4 逐项对照验收标准原文判定达成。
> 非阻塞遗留（已登记跟踪）：tls 模块 JS 面同构扩展、证书链校验（暂
> AcceptAll 对拍口径）、resolveAny 系统近似。
>
> **2026-09-09 复评登记**（证据：M3 全套 28 passed / 0 failed；评审结论见 `20260909/README.md` §10）：
> 原结项提交 `9cf1686` 将里程碑总览标 `[x]`，但细分清单从未勾选。复评发现
> M3.2（VM `tls`/`https` 模块级 TLS 接线）与 M3.4（`resolve` 家族真实递归
> 查询）**未闭环**；M3.1 / M3.3 达成。总览已改 `[~]`，以下按项登记。

- [x] **M3.1 Stream 规范级状态机与背压机制**
  - 重构 `Readable` 与 `Writable` 内部缓冲队列与水位线（`highWaterMark`）；
  - 严格支持 `pause()`、`resume()` 状态切换与 `pipe()` / `pipeline()` 背压联动；
  - 完善流异常级联自动销毁机制（`destroy(err)`）；
  - 验收：高吞吐大文件管道传输测试零内存泄漏与卡死。
  - 达成证据：`950a8a7`（水位线/writableLength/drain/pipe 联动/destroy 级联）；
    复测 `builtins_phase4_stream_test` 4/4、conformance `01-for-await-stream.cjs`
    与 `19-m4-web-streams-abort.cjs` 对拍全绿。
- [x] **M3.2 纯 Rust TLS 1.3 真实握手实现**（20260909 结项闭环）
  - 遵守静态无 C 依赖约束，引入纯 Rust `rustls` 支撑底层安全网络传输；✅ 依赖层已验证
    （`78119cc` rustls 真实 TcpStream 回环、`6cac068` crypto_provider/pem_to_der/
    make_server_config 构建层就绪，见 `builtins/tls.rs`）；
  - `https` 模块 JS 表面接入 rustls 真实会话：`https.createServer` accept 后以
    `ServerConnection` 包覆既有 HTTP 服务端处理链；`https.request` 拨号建
    `ClientConnection` 并经事件泵握手后转发 HTTP 报文（TLS 读写/`close_notify`
    收口全在 io_round 泵内调度，握手未完成不解析明文）；✅ `fb97628`
  - `tls.createServer`/`tls.connect` JS 面接线留作同构扩展（rustls 会话机制已
    下沉共用，见 M3.2b 登记收敛口径）；
  - 验收：本地真实 HTTPS 自签名证书通信回环通过。✅ `https_tls_loopback_test`
    1/1（VM https server ↔ client 自回环与 Node 22 逐字对拍；node TLS 客户端 →
    VM TLS 服务端、VM TLS 客户端 → node TLS 服务端跨实现对拍均 200）。
  - 达成证据：`fb97628`（+550/-39，7 文件）；TLS 客户端不复用 http keep-alive
    池（与 node https 默认 keepAlive=false 同语义）；证书校验暂 AcceptAll（同
    对拍探针 `rejectUnauthorized:false` 口径），证书链校验列为后续工作项。
  - **[M3.2b 登记 —— 已完成 ✅（`fb97628`）]** `tls`/`https` JS 表面接入 rustls 会话（事件泵握手调度）：
    - `https.request` 客户端：TCP 连接后经会话握手（`Connecting`→`Handshaking`）再转发 HTTP 报文；✅
    - `https.createServer`：accept 后 rustls `ServerConnection` 包覆现有 HTTP 服务端处理链；✅
    - 交付验收：JS 探针 `https.createServer(自签证书)` ↔ `https.request` 自回环与 Node 22 对拍；✅
      `https_tls_loopback_test`（含 Node 22 逐字对拍）1/1；
    - 复用 `make_server_config`/`pem_to_der` 与 `tests/conformance/node22/cases/*.pem`；✅
    - 收敛口径：以 https server/client 双向真实 TLS 闭环本工作项；`tls.connect`/
      `tls.createServer` JS 表面接线为同构扩展，留后续跟踪（会话机制已共用，无新架构风险）。
- [x] **M3.3 HTTP 1.1 生产级长连接与连接池**
  - 完整支持 HTTP 1.1 分块传输（`Transfer-Encoding: chunked`）；
  - 实现基于 `http.Agent` 的 Keep-Alive Socket 连接池复用与超时回收；
  - 验收：真实高并发 HTTP 压力测试对齐 Node.js 22 LTS。
  - 达成证据：`http/client.rs` Agent 连接池（存活连接优先复用/完整响应归还池、
    非 keep-alive 响应关闭）；chunked 编解码在 wire/fetch 层；复测
    `builtins_phase5_http_test` 10/10（含顺序多次请求 keep-alive 语义）+
    `express_e2e` 全绿。注：`https.request` 已于 M3.2 结项随 `fb97628` 接入真实
    TLS（TLS 会话不入池，与 node https 默认 keepAlive=false 语义一致）。
- [x] **M3.4 异步 DNS 解析与缓存**（20260909 结项闭环——resolve 家族真实递归查询）
  - 实现基于系统的非阻塞异步 DNS 解析（`dns.lookup`, `dns.resolve4`, `dns.promises`）；
  - 验收：域名解析集成测试稳定通过。
  - 达成面：`lookup`/`lookupService` 走系统解析（真实）；callback/promises 双面与
    事件源泵异步时序、`Resolver`/错误码/常量面齐全（复测 `builtins_phase5_net_test`
    8/8 含 dns_callback_family 与 dns_promises_family）。
  - **resolve 家族真实递归查询（M3.4b）**：接入 `hickory-proto`（纯 Rust 报文
    编解码，关默认特性不带 tokio）+ 后台 `std::thread` + `crossbeam-channel`
    桥（A+ 路线：与 VM 同步事件泵同构，避免 M5 前引入 tokio runtime；channel
    基建为 M5.1 worker_threads 复用）。callback 与 promises 双面的
    `resolve4/6/CAA/CNAME/MX/NAPTR/NS/PTR/SOA/SRV/TLSA/TXT` 与 `reverse`：
    - 本地单标签名（`localhost` 等）保持原确定性形态；非本地域名发真实
      DNS 报文（UDP 优先、截断回落 TCP、多服务器轮换、rcode→Node 错误码）；
    - 探测结果与 Node 22 实时对拍逐字一致（resolve4/NS/TXT/MX/reverse/
      promises 面/ENOTFOUND 的 code+hostname+syscall/setServers/getServers）；
    - `getServers`/`setServers` 从记录态升级为真实生效（Node 语义）。
  - **[M3.4b 登记 —— 已完成 ✅]** resolve 家族真实递归查询：接入纯 Rust DNS 客户端
    支持 resolve4/6/MX/TXT 等 rrtype，交付与 Node 22 对拍的域名解析探针。

---

### M4 · 现代 Web API 标准完全对齐
- [x] **M4.1 规范级 Fetch API 全家桶**（20260909 收尾，证据见 `20260909/README-round4.md`）
  - 完整实现全局 `fetch()`, `Request`, `Response`, `Headers`；
  - 支持 `Body` 混入（`json()`, `text()`, `arrayBuffer()`, `blob()`, `formData()`）；
  - 支持自动遵循重定向（`follow|manual|error` 三模式 ≤5 跳）、响应体解码
    （chunked 传输分帧解码）；`https://` 显式 rejected TypeError 兜底；
  - 交付：Request 继承+init 覆盖、fetch(request) 直传、Headers.get/has 大小写
    不敏感；差分 `builtins_phase9_m4_test.rs` 与 Node 22 实时双对拍逐字一致。
- [x] **M4.2 Web Streams 与 Node Streams 原生互转**（ef11dc7，20260908）
  - 规范实现 `ReadableStream`, `WritableStream`, `TransformStream`；
  - 支持 `Readable.toWeb(stream)` 与 `Readable.fromWeb(webStream)` 双向零拷贝桥接；
  - 验收：Web Streams 管道处理用例对齐 Node.js 22。
  - 交付：`Readable/Writable.fromWeb/toWeb` 四向 live 桥（挂桥补交既有队列 + 实时转发）；差分用例 `19-m4-web-streams-abort.cjs` 7 场景与 Node.js 22 逐字节一致；顺带修复 `Readable.from` 形态键缺失（静默建空流）缺陷。
- [x] **M4.3 `AbortController` / `AbortSignal` 全系统级联动**（20260909 收尾）
  - 全局注入 `AbortController` 与 `AbortSignal`；
  - 联动所有异步 I/O、HTTP Fetch 请求、定时器与网络 Socket，支持信号触发即时取消；
  - 交付：fetch 前置+后置中断；`setTimeout/setInterval/setImmediate`/
    `timers-promises` signal 取消（abort → 等价 clearTimeout）；net
    `connect/listen` signal（abort → socket 销毁 / server 关停）；差分用例
    `24-m4-timers-signal.cjs` 与 Node 22 逐字一致。
- [x] **M4.4 Web 标准事件基类与表单**（20260909）
  - `EventTarget` 与 `CustomEvent` 作为全系统事件模型抽象；
  - `FormData` 与 multipart/form-data 标准编码与分块解析；
  - 交付：EventTarget（addEventListener/removeEventListener/dispatchEvent +
    target 注入）/CustomEvent（type/detail）；FormData 全方法面（append/set[原
    位置替换]/get/getAll/has/delete/entries/keys/values/forEach）+ multipart
    编码 + urlencoded/multipart 解析（`Response.formData()`）；差分用例
    `22-m4-web-standards.cjs` 与 Node 22 逐字一致。

---

### M5 · 多线程并发与系统级扩展
> **2026-09-09 评审 + round5 结项登记**（证据见 `20260909/README.md` §15 与
> `README-round5.md`）：
> M5.1 已结项（真物理线程/postMessageToThread 通道/eval worker + Node 对拍绿）；M5.2 主体达成；**M5.3 已闭环**
> （Node 22.23.1 实测对齐——错误文本/绑定规则/columns 五键/close 语义全面
> Node 化，44 行探针逐字一致真对拍固化）；M5.4 仅 concurrency 与 Mock 达成
> （Timer Mock / 报告接线 / LCOV / CLI 运行器未闭环）。总览按项登记。
- [x] **M5.1 `worker_threads` 跨物理线程支持**（✅ 结构化克隆闭环 + 5 处语义偏离全部关闭 + 端口 `ref/unref/start/hasRef` 与 `parentPort` 方法面 + **`postMessageToThread` 真线程通路（`process.on('workerMessage')` 通道）** + **eval worker 现场编译**——两个点名缺口已于 20260911 关闭（见 [20260911/README.md §14](./20260911/README.md)），20260911 结项）
  - 基于 Rust 原生系统线程与 `crossbeam-channel` 实现真物理多线程；✅ 真
    `std::thread` + 独立 Vm（runtime 装配钩子），通道为 std mpsc（crossbeam
    仅 dns_resolver 在用；登记口径以「真线程 + 通道桥」为准）；
  - 实现 `MessageChannel`、`MessagePort` 与结构化克隆传值；✅ **结构化克隆
    闭环**（round7：worker_clone 自描述格式——基本类型/Date/RegExp/Map/Set/
    ArrayBuffer/TypedArray/DataView/**循环与共享引用**/transfer list 移交 +
    源 detach/markAsUntransferable/DataCloneError；纯消息 worker 保活修复；
    `25-m5-structured-clone.cjs` Node 22.23.1 逐字对拍 22 行一致）；
    简化口径登记：视图共享克隆独立复制、MessagePort transfer 未实现、
    workerData 复验列下轮；
  - 验收：多 Worker 并发计算与消息通信用例对拍全绿。✅ `20-m5` Node 逐字节
    对拍 PASS + phase6 3 用例 + case 25 结构化克隆对拍。缺口跟踪：eval
    worker、`postMessageToThread` 真线程分支。✅ **本轮收口**：port `ref/unref/start/hasRef`
    与 `parentPort` 方法面（Node 22 实测：ref/unref 返回 undefined、hasRef 默认 true）
    已实现并与 Node 逐字对拍；`threadId` 恒 0 的过时文件头注释已随 M5.4 轮修正。
    ✅ **20260911 结项**（证据见 [20260911/README.md §14](./20260911/README.md)）：
    `postMessageToThread` 真线程通路（Node `process.on('workerMessage')` 通道口径：
    Promise 返回 + SAME_THREAD/FAILED/ERRORED/TIMEOUT 四错误面 + worker→worker
    经主线程中转 + 真实超时到期，`37`/`38` 用例与 Node 22.23.1 逐字一致）与
    eval worker（装配层现场编译 + `__filename='[worker eval]'` + 未捕获异常
    Error 对象化 + 两类构造同步校验，`39` 用例逐字一致）全部关闭。
    **持续登记偏离**：MessagePort 经 transferList 移交未实现（仅 ArrayBuffer）；
    `Error` 族子类 `constructor.name` 恒 'Error'；语法错误消息文本为解析器自有；
    Node 对裸相对 filename 同步抛 `ERR_WORKER_PATH`（aluka 按 Go 口径接受）。
- [~] **M5.2 `cluster` 进程池模型**（端口共享 + **IPC 面最小集** + **listen 失败错误载体 `Error` 化（异步派发）** + **`settings.exec/args/silent/cwd` 生效** + **服务端 `Connection: close` 语义** + **primary 侧生命周期事件（`listening`/`disconnect`/`state`/异步 `fork`）** + **worker 侧 IPC 面（`process.on('message')` 接收 / `process.disconnect()` / `process` 真实事件器 / `cluster.worker` 事件面与桥接 / 通道默认保活）** + **`process.channel` 对象面（`ref`/`unref`/`refCounted`/`unrefCounted`/`fd`）** + **断连闭环（primary `{"t":"d"}` 帧路径 + worker `cluster.worker.disconnect()` + 断连关闭 worker 内 server）** 达成——余 RR 调度（架构级，见 §10.6/§11.5 决策记录），20260911）
  - 实现 Master / Worker 进程拓扑与 IPC 通道分发套接字；⚠️ 真多进程拓扑
    （self-exe spawn + `ALUKA_WORKER_ID`）+ socket2 SO_REUSEADDR/REUSEPORT
    OS 内核分发（非 IPC 句柄传递）；⚠️ 该三项已于 20260910 收口——
    `worker.send`/`process.send`/`cluster.worker.send` 真实可用、`isConnected()`/`isDead()`
    与 `'exit'` 退出码均为真实值（余：RR 调度）；
    无 RR 调度（内核对分发）；`worker.send`/`isConnected`/`isDead`/exit code 已于 20260910 收口；
    `settings.exec/args/silent/cwd` 生效 + settings 契约（默认值/浅合并/对象重建 +
    `fork` 隐式 `setupPrimary`）+ validator 文本面已于 20260911 收口（详见
    [20260911/README.md §7](./20260911/README.md)）；
  - 验收：多进程集群 HTTP 端口共享测试通过。✅ `21-m5` Node 逐字节对拍 PASS
    （bc 模式实测）。✅ **P0 已关闭**（round6：fetch 响应完成判定不依赖
    连接关闭——原「挂死」实为每请求 10s 读超时叠加，修复后并发双 fetch
    20s → 13ms、phase9 71s → 1.1s；conformance 全量绿）。✅ **服务端
    `Connection: close` 已闭环**（20260911）：请求 `close`／响应显式设 `close`／
    HTTP/1.0 无 keep-alive／HTTP/1.0 + keep-alive → 响应带 `Connection: close`
    且落盘后发 FIN；其余写 `keep-alive` + `Keep-Alive: timeout=5`。副作用对齐：
    HTTP/1.0 客户端响应不写 `Content-Length`（关连接定界）。五情形原始报文 +
    连接复用两组探针与 Node 逐字节一致（详见
    [20260911/README.md §8](./20260911/README.md)）。✅ **primary 侧生命周期事件
    已闭环**（20260911）：`worker.state` 全生命周期（`none→online→listening→
    disconnected→dead`）、`cluster.on('listening', (worker, info))`（2 实参 +
    键集 `addressType/address/port/fd`）、`cluster.on('disconnect', (worker))`
    （1 实参、仍在 `workers` 表）、`'fork'` 异步化（`nextTick`）与
    `isConnected()` 改为通道连通性语义；四形态 listen payload（显式 IP／
    `0.0.0.0`／`::1`／未指定 host → `address=null`）逐字节对拍一致。**顺带修复
    `cluster_ipc` 握手读超时误判导致的偶发 IPC 面静默失效（约 1.5% 复现率 →
    0/250）**。✅ **worker 侧 IPC 面已闭环**（20260911 §10）：`process` 事件面由空实
    现改为真实事件器（`on`/`addListener`/`once`/`off`/`removeListener`/
    `removeAllListeners`/`emit`/`listenerCount`/`listeners`，别名同一函数对象）；
    primary → worker 消息投递（2 实参、`handle` 恒 `undefined`）；`process.disconnect()`
    （返回 `undefined`、`connected` 同步翻转、二次调用 `ERR_IPC_DISCONNECTED`、
    `'disconnect'` 经 `nextTick` 异步派发）；worker 侧 `cluster.worker.on('message')`/
    `send`/`isConnected`/`isDead` 与 `require('cluster')` 时的 process→worker 桥接；
    **IPC 通道默认保活**（Node 实测口径：fork 出的子进程脚本跑完不退出）——
    5 例探针与 Node v22.23.1 逐字节一致（详见
    [20260911/README.md §10](./20260911/README.md)）。✅ **`process.channel` 对象面已闭环**
    （20260911 §11，`61da226`）：`ref`/`unref`/`refCounted`/`unrefCounted` + `fd` 自有键；
    `unref()` **真正解除通道保活**（排空后 worker 以 code 0 自然退出）、同 tick `unref→ref`
    可恢复（`fd` 值与 `Control` 类名为登记偏离）。✅ **断连闭环已达成**
    （20260911 §12，`7012ce0`）：primary 侧 `intercom`/`removeWorker`/`Worker.prototype.disconnect`
    （置 `ead=true` → 发 `{"t":"d"}` 帧 → **立即出表** → 返回 `this`）+ `cluster.disconnect(cb)`
    重写（不再 `destroy` 杀进程；workers 为空走 `nextTick` 触发 cb）；worker 侧
    `cluster.worker.disconnect()`（返回自身、**同步**置 `state='disconnecting'`/`ead=true`、
    **同步**关闭本进程内 server）；配套新增 `net`/`http` 的 `pub(crate)` 批量关闭
    （断连关闭 worker 内全部监听 server 并派发 `'close'`，否则 worker 无法优雅退出）。
    两例 e2e（`m52_disconnect_test.rs`）与 Node v22.23.1 逐字节一致。
    ✅ **`{"t":"e"}` ack 回程已闭环**（20260911 待办 31，证据见
    [20260911/README.md §15](./20260911/README.md)）：worker 上报后**挂起**
    （`process.connected` 同步段保持 `true`——Node 实测口径，修复前为 false 即
    偏离点），primary 置 `ead=true` 后回同帧 ack（Node `{ack: seq}` 的无 seq
    近似），worker 收到才收尾 `process.disconnect()`；上报失败立即收尾、挂起中
    重复调用 no-op、对端 EOF 挂起失效。第 3 例 e2e 逐字对拍。
    ✅ **RR 调度决策已正式化（20260912，维持偏离结项）**：`SCHED_RR` 需 unsafe FFI
    与仓库 `unsafe_code=deny` 冲突；fd 传递介质在 Windows 不可用且违反零依赖分发；
    SO_REUSEPORT 内核分发的可观测差异仅限连接粘性（21-m5 差分全绿）。详见
    [20260912/README.md §3](./20260912/README.md)。
    遗留：`Object.keys` 键序（字典序 vs 插入序）为独立
    全仓专项（详见 [20260911/README.md §9](./20260911/README.md)）。
- [x] **M5.3 `node:sqlite` 生产级支持**（✅ Node 22.23.1 实测对齐 + 真对拍闭环，20260909 round5）
  - 规范实现 `DatabaseSync` 类与 SQL 语句 `StatementSync`；⚠️ 非真预编译
    （每次执行重编译，语义等价）登记跟踪；`columns()` 对齐 Node 五键
    （column/database/name/table/type，表达式列 null）；无 ctor options 登记；
    错误文本全面 Node 化（message=errmsg 原文；code ERR_SQLITE_ERROR +
    errcode 扩展码 + errstr；TypeError 挂 ERR_INVALID_ARG_TYPE）；
  - 支持事务控制（`BEGIN`, `COMMIT`, `ROLLBACK`）与复杂类型映射；✅ exec
    直写 + `isTransaction` 与 Node 22 实测一致；bigint/blob→Uint8Array/
    Boolean→TypeError/缺参 NULL 补/超位越界/Unknown named parameter 对齐；
    ⚠️ **`db.transaction(fn)` 为超集扩展**——Node 22 LTS 原型面无此方法
    （Node 23.8+ 才有），wrapper 与 isTransaction 同步登记跟踪；
  - 验收：对齐 Node 22 原生 SQLite 操作测试。✅ 新增 `sqlite_node22_diff_
    e2e_matches_node` 真对拍（`probes/node22_sqlite_probe.js` 44 行输出与
    Node 22.23.1 **逐字一致**）+ 既有 4 用例断言更新；裸名 `require('sqlite')`
    可用（剥前缀折衷已登记）。**遗留**：ctor options、真预编译句柄语义、
    wrapper 事务的 isTransaction 同步。
- [x] **M5.4 `node:test` 进阶测试套件**（切片一（模块形态 / 函数属性 / CLI 运行器）+ 切片二（Timer Mock）+ ✅ 真 `stream.Transform` 报告器（`run().compose(reporter).pipe(dest)`）+ ✅ **LCOV 行覆盖**（`aluka test --test-reporter=lcov`：SpannedStmt 行号 → 编译期行表 → VM 逐行计数 → tracefile 生成四层闭环）——20260912 结项，证据见 [20260912/README.md](./20260912/README.md)）
  - ✅ **切片一（20260910，证据见 `20260910/README.md` 待办 24 + `crates/aluka-cli/tests/test_runner_cli_test.rs` 6 例）**：
    `it`/`test`/`describe`/`suite` 的 `skip`/`todo`/`only` **函数属性形态**；
    **`require('node:test')` 的导出值改为可调用的 `test` 函数**（Node 22 实测口径：
    `typeof === 'function'`、`t.it === t`、`t.test === t`、`t.describe === t.suite`）——
    修前是普通对象，导致 `const test = require('node:test'); test(name, fn)` 这一
    Node 最常见写法完全不可用；`test.todo(name)` 无回调不再执行且记 `ok … (TODO)`、
    有回调执行但 `fail` 不计入；CLI **`aluka test [--test-reporter=<spec|tap|dot>] [目标...]`**
    （目录递归发现、忽略 `node_modules`、每文件独立 Runtime、失败退码 1）把报告器纯函数
    接到生产路径。**报告格式沿用本仓 Go CLI 契约，不声称与 `node --test` reporter 逐字一致。**
  - ❌ **仍未闭环**：LCOV 覆盖率（成本已量化——需 AST 位置 → 编译期行号表 → VM 逐行计数 → LCOV 生成四层改造，见 `20260910/README.md` 待办 25）。
    ✅ **真 `stream.Transform` 报告器已闭环**（20260911 待办 32）：`stream.Transform`
    原生构造器（真 prototype 链，`instanceof` 成立）；报告器实例升级为 Transform
    实例（`SpecReporter` 等构造名 + `writableObjectMode` + 导出名怪癖对齐 Node 实测）；
    **`run().compose(reporter).pipe(process.stdout)` 可用**（tap：`TAP version 13` 头 +
    `ok N - name` + YAML 块 + `1..N` + `# tests` 汇总；spec/dot 同理），事件增量格式化、
    pipe 直通/补冲；输出文本沿用本仓报告契约（不与 `node --test` reporter 逐字——
    既定口径），tap/dot/junit 工厂 `new` 不复刻 Node 的 TypeError 怪癖（登记）。
  - 支持并发测试执行（`concurrency` 选项）；✅ 单线程 async 交错（与 Node
    协作式并发语义一致），phase8 e2e 绿；
  - 支持函数/方法 Mock、Timer Mock 推进；✅ Mock 族（fn/method/getter/
    setter/property + spy.mock.calls）`13-mock.cjs` Node 逐字节一致；✅ **Timer Mock 已实现**
    （20260910 切片二：`enable`/`tick`/`setTime`/`runAll`/`reset`，与 Node 逐字对拍一致；
    `apis:['Date']` / `['scheduler.wait']` 未实现已登记）；
  - 支持 Spec、TAP、LCOV 覆盖率报告生成；✅ spec/tap/dot 格式化纯函数**已接线到生产路径**
    （`aluka test --test-reporter=...`）、报告器 `write` 已转发 `data`（不再吞数据）；
    ❌ LCOV 仍无实现；
  - 验收：官方 node:test 兼容性测试套件全量通过。❌ 本仓无该套件；语料
    15/16 因含故意失败用例被判 INVALID 从未真对拍；`test.skip` 等函数属性形态
    **已实现**（20260910 切片一）。**未做**：无官方套件、`//@test` 语料仍未纳入差分对拍。

---

### M6 · 生产级 GC 与高性能引擎
- [x] **M6.1 生产级分代 GC 闭环**（✅ 卡表写屏障 + 自适应堆伸缩 + 生产双代 + GC 压力模式已合入主循环（commits 5f7289f / 59a13e4）；✅ 验收：`gcPressure` 内存基准 **1.35x**（对标 Node.js 22 (V8) 峰值内存，远优于 2~3x 验收线）——20260912 核验补登记，证据 `crates/aluka-cli/examples/gcpressure.rs` 与 `.work/TODO/20260912/README.md`）
- [x] **M6.2 8 字节 NaN-boxing `Value` 切换**（20260912 分支 m62-nanobox 落地并合并，
  收尾提交 7c74aef；证据 `.work/TODO/20260912/README.md` §7）
  - ✅ `Value` 表示切换完成：`crates/aluka-vm/src/value.rs` 重写为 8 字节 NaN-box
    机器字（`#[repr(transparent)] u64`，`size_of==8` 编译期断言），编码与
    `aluka-jit/src/valbox.rs` 同源（f64 比特直存 + `0xFFF7…|tag`，ObjectRef 占
    8..=39 位）；兼容层（关联常量/同名构造函数/`ValueCase` 镜像 + 访问器）使
    全仓 4900+ 处 `Value::` 引用完成迁移；
  - ✅ 门禁全绿：fmt / clippy `-D warnings` 0 错；全量测试 **639 passed, 0 failed**；
    `ALUKA_GC_STRESS=8` **545 passed, 0 failed**；conformance 全量差分 vs
    node v22.23.1 逐字节一致；jitdiff 逐位一致全绿（JIT/解释器共享值域根基）；
  - ✅ 内存收益兑现：gcPressure **1.25x**（M6.1 时 1.35x → 8 字节堆峰值下降）；
  - ⚠️ 吞吐复合验收待 M6.3：fib_bench 单项 1.019x（824.5ms vs 840.4ms，负载以
    269 万次调用压栈为主，表示切换直接收益有限）；总表「≥1.5x」为
    **表示切换 + M6.3 PIC/JIT 协同**的复合目标，验收线不放宽，M6.3 完成后复核；
  - 附带修复：`aluka-core::Value::is_object` 无限递归；`test/state.rs` GC 重入
    `borrow_mut`（分配移出借锁——`ALUKA_GC_STRESS=8` 下确定性 panic 的 M5 潜伏缺陷）。
- [ ] **M6.3 多态内联缓存 (PIC) 与 JIT 全指令流扩容**
  - 对象属性存取、方法调用、局部变量读写全量接入多态 Shape 内联缓存；
  - 扩充 Cranelift JIT 后端支持更丰富的控制流与调用指令发射；
  - 验收：密集计算与循环调用基准测试显著超越解释器基线。
  - 📌 现状登记（20260912）：JIT Cranelift 后端（J2 数值子集）+ `PicCell`
    形状缓存结构 + `jit_hot` 热点分层**已落地**；`jitbench` 3/3 PASS
    （hot_loop/prop_sum PIC/closure_call——保守门禁「JIT 不慢于解释器」已固化）。
  - ✅ 切片一（20260912，715dd43）：**解释器属性读取 IC 落地**——4096 槽
    direct-mapped 站点缓存（`pic.rs`，GetProp/GetPropLocal），6 条命中守卫 +
    魔法键写回资格，语义与慢路径逐条对齐；7 项单测 + 全仓 644 测试 +
    GC 压力全绿；fib_bench 824.5→812.9ms（M6.2 以来累计 1.034x）。
    剩余：写路径/方法调用 IC + 多态桩（≈1 天）；JIT 扩容至调用/闭包/
    生成器/Try 全指令流（≈2~3 天，涉及调用约定与 GC 栈映射协同）——
    后续专项轮次继续。

---

### M7 · 终局合并与全面验收 (Node.js 22 全面对齐)
- [ ] **M7.1 运行时程序合并（流程不变）**
  - 将 `alukac` 与 `aluvm` 合并为统一的 `aluka` 单二进制发布形态；
  - 提供 `aluka run <file>`（自动先编译/校验再执行）与 `aluka build`（打包）统一 CLI 交互体验；
  - 严格保持内部源码 → 字节码校验 → VM 解释执行的分层流水线不变；
  - 验收：单二进制独立分发与跨平台执行验证通过。
- [ ] **M7.2 Node.js 22 官方 Conformance 规模化通过**
  - 扩容 Node.js 22 LTS 官方对拍语料库至 ≥1000 个核心用例；
  - 自动化差分对拍达到 100% 预期一致性；
  - 验收：大规模集成测试对拍全绿。
- [ ] **M7.3 npm Top 50 真实生态包无缝运行签核**
  - 选取主流流行包（Express, Lodash, Chalk, Zod, Commander, Dotenv 等）；
  - 100% 无报错通过其自带的端到端测试；
  - 验收：签发 Node.js 22 (LTS) 生产级全面兼容证书。
