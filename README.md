# Aluka

Aluka 是一个纯 Rust 实现的高性能、现代化 JavaScript 与 TypeScript 运行时引擎。
致力于实现毫秒级超快冷启动、极低常驻内存开销以及与 **Node.js 22 (LTS)** 标准生态的深度兼容。

工程协作与规范指南请参阅 [AGENTS.md](./AGENTS.md)。

---

## 一、项目目标定义

### 1. 核心愿景
- **毫秒级冷启动**：专为边缘计算、轻量微服务与高性能嵌入式脚本任务设计；
- **极低常驻内存**：自管紧凑内存模型，消除托管运行时垃圾回收开销；
- **Node.js 22 (LTS) 深度兼容**：以官方 Node.js 22 LTS (v22.23.1+) 行为与 ECMAScript / test262 规范为唯一权威 oracle；
- **纯净单二进制**：静态单二进制交付，零外部运行时依赖。

### 2. 核心架构设计原则
1. **字节码作为 ISA 契约**：
   编译前端（`alukac`）与执行后端（`aluvm`）完全解耦，以强类型、静态验证的 Aluka 字节码指令集（ISA）为唯一交互契约。
2. **确定性语义保障**：
   使用 Node.js 22 标准套件与 test262 进行端到端差分对拍，杜绝语义偏离。
3. **零内存安全隐患**：
   全仓默认禁用 `unsafe`（`unsafe_code = "deny"`），确保内存绝对安全。

### 3. 架构演进路线与形态规划
- **当前开发模式（前后端解耦）**：当前阶段严格作为**编译前端（`alukac`）与执行后端（`aluvm`）**两个独立组件与二进制进行开发与对拍，前后端以标准 ISA 字节码严格解耦，保障架构边界清晰与模块独立演进；
- **终局合并方案（流程不变）**：在后续达成对 Node.js 22 全面兼容的既定目标之后，考虑进行程序合并（提供开箱即用的统一单二进制体验），同时保持**内部编译与执行分层流程不变**（源码 → 字节码生成/静态校验 → VM 执行的分层流水线与 ISA 契约保持绝对稳定）。

---

## 二、工作流定义

### 1. 开发与验证工作流

基线工具链：`Rust 1.85+`（推荐 `1.95+`）。

```bash
# 1. 源码构建流程
cargo build

# 2. 全套测试与差分对拍工作流
cargo test --workspace --all-features

# 3. 静态分析与代码门禁工作流
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

### 2. 编译与运行工作流

```bash
# 步骤 1：使用前端编译器（alukac）将 JS/TS 编译为字节码
cargo run -p aluka-cli --bin alukac -- app.js -o app.bc

# 步骤 2：使用执行后端虚拟机（aluvm）运行字节码
cargo run -p aluka-cli --bin aluvm -- run app.bc

# 步骤 3（可选）：反汇编与自检字节码结构
cargo run -p aluka-cli --bin alukac -- disasm app.bc
```
