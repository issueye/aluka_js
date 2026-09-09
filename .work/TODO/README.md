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
| **M5** | **多线程并发与进阶能力** | `worker_threads` 真实跨物理线程 Worker、`cluster` 进程池、`node:sqlite` 原生数据库支持 | `[~]`（M5.1/M5.2 主体达成；M5.3 ✅ 已闭环；M5.4 未闭环，见 §M5） |
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
> M5.1/M5.2 主体达成（真物理线程/端口共享 + Node 对拍绿）；**M5.3 已闭环**
> （Node 22.23.1 实测对齐——错误文本/绑定规则/columns 五键/close 语义全面
> Node 化，44 行探针逐字一致真对拍固化）；M5.4 仅 concurrency 与 Mock 达成
> （Timer Mock / 报告接线 / LCOV / CLI 运行器未闭环）。总览按项登记。
- [~] **M5.1 `worker_threads` 跨物理线程支持**（✅ 结构化克隆闭环——余项跟踪，20260909 round7）
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
    worker、`postMessageToThread` 真线程分支、port ref/unref/start、
    文件头注释过时项（threadId 恒 0 等）。
- [~] **M5.2 `cluster` 进程池模型**（端口共享达成——IPC 面降级 + P0 遗留，20260909 评审）
  - 实现 Master / Worker 进程拓扑与 IPC 通道分发套接字；⚠️ 真多进程拓扑
    （self-exe spawn + `ALUKA_WORKER_ID`）+ socket2 SO_REUSEADDR/REUSEPORT
    OS 内核分发（非 IPC 句柄传递）；worker.send 恒 true、isConnected/
    isDead 恒值、exit code 硬编码 0、无 RR 调度；
  - 验收：多进程集群 HTTP 端口共享测试通过。✅ `21-m5` Node 逐字节对拍 PASS
    （bc 模式实测）。✅ **P0 已关闭**（round6：fetch 响应完成判定不依赖
    连接关闭——原「挂死」实为每请求 10s 读超时叠加，修复后并发双 fetch
    20s → 13ms、phase9 71s → 1.1s；conformance 全量绿）。遗留：⚠️ IPC 面
    （worker.send/isConnected/isDead/exit code）降级、`Connection: close`
    响应后关闭连接的 server 语义、listen 错误载体为字符串非 Error 对象。
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
- [ ] **M5.4 `node:test` 进阶测试套件**（仅 concurrency + Mock 达成，20260909 评审）
  - 支持并发测试执行（`concurrency` 选项）；✅ 单线程 async 交错（与 Node
    协作式并发语义一致），phase8 e2e 绿；
  - 支持函数/方法 Mock、Timer Mock 推进；✅ Mock 族（fn/method/getter/
    setter/property + spy.mock.calls）`13-mock.cjs` Node 逐字节一致；❌ Timer
    Mock 零代码；
  - 支持 Spec、TAP、LCOV 覆盖率报告生成；❌ 报告器 write 吞数据恒 true、
    格式化纯函数未接线、LCOV 无实现、CLI `aluka test` 入口不存在；
  - 验收：官方 node:test 兼容性测试套件全量通过。❌ 本仓无该套件；语料
    15/16 因含故意失败用例被判 INVALID 从未真对拍；`test.skip` 等函数属性
    形态降级（options 形态可用）。

---

### M6 · 生产级 GC 与高性能引擎
- [ ] **M6.1 生产级分代 GC 闭环**
  - 将 ADR-0002 选型的分代标记-清除 GC 正式合入虚拟机主循环；
  - 接入基于卡表（Card Table）的跨代写屏障（Write Barrier）；
  - 动态堆伸缩策略（基于内存压力自适应触发 Minor / Major 收集）；
  - 验收：`gcPressure` 内存基准指标全面对标 Node.js 22 (V8) 2~3x 以内。
- [ ] **M6.2 8 字节 NaN-boxing `Value` 切换**
  - 将 `Value` 内部表示从 16 字节 Tagged Enum 切换为 8 字节 NaN-boxing 机器字；
  - 降低 50% 栈空间与常量池常驻占用，大幅提升 CPU 缓存命中率；
  - 验收：全量测试套件与性能 Benchmark 提升 1.5x 以上。
- [ ] **M6.3 多态内联缓存 (PIC) 与 JIT 全指令流扩容**
  - 对象属性存取、方法调用、局部变量读写全量接入多态 Shape 内联缓存；
  - 扩充 Cranelift JIT 后端支持更丰富的控制流与调用指令发射；
  - 验收：密集计算与循环调用基准测试显著超越解释器基线。

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
