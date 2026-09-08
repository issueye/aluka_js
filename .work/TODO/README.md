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
| **M3** | **核心内置模块生产级闭环** | Stream 规范背压状态机、纯 Rust TLS 1.3 握手、HTTP 1.1/2 Keep-Alive 连接池、异步 DNS | `[ ]` |
| **M4** | **现代 Web API 标准对齐** | 规范级 Fetch API、Web Streams 与 Node Streams 原生互通、`AbortController` 全系统级联动中断 | `[ ]` |
| **M5** | **多线程并发与进阶能力** | `worker_threads` 真实跨物理线程 Worker、`cluster` 进程池、`node:sqlite` 原生数据库支持 | `[ ]` |
| **M6** | **生产级 GC 与高性能引擎** | 分代标记-清除 GC 正式合入主流程、8 字节 NaN-boxing 切换、多态内联缓存（PIC）与 JIT 全指令流扩容 | `[ ]` |
| **M7** | **终局合并与全面验收** | `alukac` 与 `aluvm` 合并为统一 `aluka` 单二进制（流程不变）、Node.js 22 官方套件 ≥1000 例全绿通过 | `[ ]` |

---

## 3. 里程碑细分待办清单

### M1 · ECMAScript 核心规范收口 (ES2024 / test262)
- [ ] **M1.1 Proxy & Reflect 反射代理子系统**
  - 实现 `Proxy` 构造器与 13 种核心 Traps 拦截器（`get`, `set`, `has`, `deleteProperty`, `apply`, `construct`, `getPrototypeOf`, `setPrototypeOf`, `isExtensible`, `preventExtensions`, `getOwnPropertyDescriptor`, `defineProperty`, `ownKeys`）；
  - 实现全局 `Reflect` 对象的 13 个静态规范方法；
  - 验收：通过 Proxy/Reflect 专属测试套件（≥40 用例）。
- [ ] **M1.2 RegExp 引擎进阶语法**
  - 正向后行断言 `(?<=...)` 与负向后行断言 `(?<!...)`；
  - 命名捕获组反向引用 `\k<name>` 与替换语法 `$<name>`；
  - 验收：RegExp 语法矩阵 100% 与 Node.js 22 LTS 差分对齐。
- [ ] **M1.3 ES2022~ES2024 新增标准方法**
  - `Promise.withResolvers` 规范实现；
  - 数组不可变变更方法：`toSorted()`, `toReversed()`, `toSpliced()`, `with()`；
  - 分组方法：`Object.groupBy()`, `Map.groupBy()`；
  - 字符串 Well-Formed 校验与转换：`isWellFormed()`, `toWellFormed()`；
  - 验收：单测与 Node.js 22 对拍一致。
- [ ] **M1.4 类型化数组 (TypedArray) 规范体系**
  - 完备的 `ArrayBuffer`、`SharedArrayBuffer`、`DataView` 内存操作；
  - 11 种 TypedArray 构造函数与完整原型链继承关系；
  - 验收：TypedArray 二进制存取与字节序测试全绿。
- [ ] **M1.5 test262 官方测试集扩容**
  - 接入官方 test262 标准测试 runner，测试集规模从 8 例扩容至 ≥100 例；
  - 验收：`cargo test -p aluka-cli --test test262_subset_test` 100% 通过。
- [ ] **M1.6 动态代码求值子系统 (`eval` & `new Function`)**
  - **直接调用 `eval(code)` (Direct Eval)**：实现调用栈帧与局部词法作用域穿透，支持在当前局部环境内即时求值；
  - **间接调用 `eval(code)` (Indirect Eval)**：严格限制在全局作用域下求值，隔离局部调用帧；
  - **动态函数构造器 `new Function(...args, body)` / `Function(...)`**：实现形参与函数体字符串拼接解析、全局作用域函数模板动态生成；
  - **动态字节码 Verifier 安全门禁**：动态编译产出的字节码必须 100% 经由 `aluka-bytecode::verifier` 静态安全校验，杜绝非法跳转与栈溢出；
  - 验收：通过 eval 与 Function 专项测试套件（≥50 用例），与 Node.js 22 LTS 差分对拍 100% 一致。

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
- [ ] **M3.1 Stream 规范级状态机与背压机制**
  - 重构 `Readable` 与 `Writable` 内部缓冲队列与水位线（`highWaterMark`）；
  - 严格支持 `pause()`、`resume()` 状态切换与 `pipe()` / `pipeline()` 背压联动；
  - 完善流异常级联自动销毁机制（`destroy(err)`）；
  - 验收：高吞吐大文件管道传输测试零内存泄漏与卡死。
- [ ] **M3.2 纯 Rust 原生 TLS 1.3 握手实现**
  - 遵守静态无 C 依赖约束，引入纯 Rust `rustls` 支撑底层安全网络传输；
  - 实现 `tls.createServer` 与 `tls.connect` 真实 TLS 握手与 ALPN 协商；
  - 实现 `https` 模块生产级客户端请求与服务端监听；
  - 验收：本地真实 HTTPS 自签名证书通信回环测试通过。
- [ ] **M3.3 HTTP 1.1 生产级长连接与连接池**
  - 完整支持 HTTP 1.1 分块传输（`Transfer-Encoding: chunked`）；
  - 实现基于 `http.Agent` 的 Keep-Alive Socket 连接池复用与超时回收；
  - 验收：真实高并发 HTTP 压力测试对齐 Node.js 22 LTS。
- [ ] **M3.4 异步 DNS 解析与缓存**
  - 实现基于系统的非阻塞异步 DNS 解析（`dns.lookup`, `dns.resolve4`, `dns.promises`）；
  - 验收：域名解析集成测试稳定通过。

---

### M4 · 现代 Web API 标准完全对齐
- [ ] **M4.1 规范级 Fetch API 全家桶**
  - 完整实现全局 `fetch()`, `Request`, `Response`, `Headers`；
  - 支持 `Body` 混入（`json()`, `text()`, `arrayBuffer()`, `blob()`, `formData()`）；
  - 支持自动遵循重定向、流式下载响应体；
  - 验收：Fetch 规范测试套件通过。
- [ ] **M4.2 Web Streams 与 Node Streams 原生互转**
  - 规范实现 `ReadableStream`, `WritableStream`, `TransformStream`；
  - 支持 `Readable.toWeb(stream)` 与 `Readable.fromWeb(webStream)` 双向零拷贝桥接；
  - 验收：Web Streams 管道处理用例对齐 Node.js 22。
- [ ] **M4.3 `AbortController` / `AbortSignal` 全系统级联动**
  - 全局注入 `AbortController` 与 `AbortSignal`；
  - 联动所有异步 I/O、HTTP Fetch 请求、定时器与网络 Socket，支持信号触发即时取消；
  - 验收：超时中止与手动中断场景对拍一致。
- [ ] **M4.4 Web 标准事件基类与表单**
  - `EventTarget` 与 `CustomEvent` 作为全系统事件模型抽象；
  - `FormData` 与 multipart/form-data 标准编码与分块解析；
  - 验收：Web API 标准符合性测试通过。

---

### M5 · 多线程并发与系统级扩展
- [ ] **M5.1 `worker_threads` 跨物理线程支持**
  - 基于 Rust 原生系统线程与 `crossbeam-channel` 实现真物理多线程；
  - 实现 `MessageChannel`、`MessagePort` 与结构化克隆传值；
  - 验收：多 Worker 并发计算与消息通信用例对拍全绿。
- [ ] **M5.2 `cluster` 进程池模型**
  - 实现 Master / Worker 进程拓扑与 IPC 通道分发套接字；
  - 验收：多进程集群 HTTP 端口共享测试通过。
- [ ] **M5.3 `node:sqlite` 生产级支持**
  - 规范实现 `DatabaseSync` 类与 SQL 语句预编译 `StatementSync`；
  - 支持事务控制（`BEGIN`, `COMMIT`, `ROLLBACK`）与复杂类型映射；
  - 验收：对齐 Node 22 原生 SQLite 操作测试。
- [ ] **M5.4 `node:test` 进阶测试套件**
  - 支持并发测试执行（`concurrency` 选项）；
  - 支持函数/方法 Mock、Timer Mock 推进；
  - 支持 Spec、TAP、LCOV 覆盖率报告生成；
  - 验收：官方 node:test 兼容性测试套件全量通过。

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
