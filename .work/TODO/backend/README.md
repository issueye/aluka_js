# 后端虚拟机（aluvm）专属任务清单

> **责任域**：`aluka-core`（Value 表示、堆管理、隐藏类 Shape、GC 原型）、`aluka-vm`（Verifier 字节码校验、指令解释执行、调用帧、异常展开）、`aluka-builtins`（`node:*` 系列内置库）、`aluka-webapi`（现代 Web 标准 API）、`aluvm` 命令行工具。  
> **交互契约**：仅依赖 `aluka-bytecode` 接收字节码，严禁直接依赖前端 AST 或 Parser 类型。  
> **验证闭环**：以 Node.js 22 LTS 官方标准输出为唯一差分对拍真理，保证逐字符行为等价。

---

## 一、当前演进任务 (对齐 Node.js 22 LTS 运行时)

### 1. 核心虚拟机与底层系统 (`aluka-core` / `aluka-vm`)
- [x] **Proxy & Reflect 运行期实现**：
  - 在 `Value` / `Object` 体系中增加 Proxy 内部槽与 13 种 traps 调用派发；
  - 实现 `Reflect` 对象全套 13 个静态方法；
- [ ] **生产级分代 GC 闭环 (M6)**：
  - 将 ADR-0002 胜出的分代标记-清除 GC 正式合入主循环；
  - 引入跨代卡表（Card Table）写屏障，降低垃圾回收暂停时间；
- [ ] **8 字节 NaN-boxing `Value` 切换**：
  - 落地 8 字节 NaN-box 机器字表示，优化寄存器压栈与内存占用；
- [ ] **多态内联缓存 (PIC) 与 JIT 全覆盖**：
  - 扩展 Shape PIC 覆盖对象读写、原型链遍历与函数多态调用；
  - 扩容 Cranelift JIT 后端机器码发射能力；
- [x] **运行时动态求值执行驱动 (eval & Function)**：
  - 设计并接入运行时编译器 Hook（Runtime Compiler Hook / Eval Provider），在保持 ISA 解耦前提下接收动态字节码；
  - 动态字节码强制执行 Verifier 即时静态安全校验，严防恶意或破损字节码破坏 VM 不变量；
  - 直接调用（Direct Eval）：支持访问与穿透当前调用栈帧的局部环境与作用域字典；
  - 间接调用（Indirect Eval）与 `new Function`：强制在全局作用域与独立调用帧中执行。

### 2. 核心内置模块生产级推进 (`aluka-builtins`)
- [x] **动态求值核心内置对象 (eval & Function)**：
  - 规范实现全局 `eval(x)` 函数，支持区分直接/间接调用上下文；
  - 规范实现全局 `Function` 构造函数、原型链与严格模式限制属性（`caller`/`arguments`）；
- [ ] **Stream 流规范背压状态机**：
  - 彻底重构 `Readable` / `Writable` 内部缓冲水位线（`highWaterMark`）与背压联动；
  - 支持 `pipeline()` 自动资源释放与错误级联传播；
- [ ] **生产级 TLS 1.3 握手与 HTTPS (纯 Rust `rustls`)**：
  - 引入静态纯 Rust `rustls` 提供原生安全传输层支持；
  - 实现真实客户端 HTTPS 请求与服务端监听（非 Mock/表面桩）；
- [ ] **HTTP 1.1/2 长连接与连接池**：
  - 实现 `http.Agent` 连接复用与 Keep-Alive 调度；
  - 真实跑通 Express 真实依赖树与 Web 服务场景；
- [ ] **多线程 Worker (`worker_threads`)**：
  - 基于 Rust 物理线程与 `crossbeam-channel` 实现真实的并发 Worker 线程与 MessagePort；
- [ ] **进阶内置库**：
  - `node:sqlite` 原生数据库支持；
  - `node:test` 异步并发与 TAP/LCOV 报告器。

### 3. 现代 Web API 对齐 (`aluka-webapi`)
- [ ] **规范级 Fetch API**：`fetch()`, `Request`, `Response`, `Headers` 全功能流式支持；
- [ ] **Web Streams 互转**：`ReadableStream`, `WritableStream`, `TransformStream` 与 Node Stream 双向桥接；
- [ ] **全系统级 `AbortController` 联动**：网络与异步 I/O 支持随时取消。
