# 前端编译器（alukac）专属任务清单

> **责任域**：`aluka-parser`（词法分词、AST 构建、TypeScript 类型剥离）、`aluka-compiler`（作用域分析、指令发射、MaxStack 分析、常量折叠与优化）、`alukac` 命令行工具。  
> **交互契约**：仅依赖 `aluka-bytecode` 定义的强类型指令集与元数据，严禁跨层引入后端运行时类型。  
> **验证闭环**：前端产出的字节码必须 100% 通过 `aluka-bytecode::verifier` 静态安全性校验。

---

## 一、当前演进任务 (对齐 Node.js 22 LTS 语法面)

### 1. 语法解析与 AST 扩展 (`aluka-parser`)
- [ ] **Proxy & Reflect 语法层面适配**：确保相关的构造与成员调用无歧义；
- [ ] **RegExp 进阶语法解析**：支持 Lookbehind 正反向断言 `(?<=...)` / `(?<!...)`、命名组引用 `\k<name>`；
- [ ] **Top-Level Await (TLA) 解析支持**：
  - 识别模块顶层 `await` 语法；
  - 在 `SourceUnit` 正确打上 `has_tla: true` 阶段位；
- [ ] **现代语法边界完善**：
  - 严格模式（Strict Mode）全语法检查；
  - 增强对象/数组深度解构与默认值赋值；
- [ ] **动态代码求值语法支持 (eval & Function)**：
  - 静态识别直接 `eval(...)` 调用形态，在 AST 作用域打上 `has_direct_eval` 标记；
  - 提供 `Function` 构造器形参列表与函数体字符串的专属解析通道。

### 2. 代码生成与字节码优化 (`aluka-compiler`)
- [ ] **Top-Level Await 字节码发射**：生成支持异步模块加载的字节码结构；
- [ ] **常量折叠与 Dead Code Elimination (DCE)**：编译期静态折叠字面量二元运算，消除不可达分支；
- [ ] **MaxStack 深度精准推导**：完善嵌套闭包与复杂三元表达式的最大操作数栈深度推导，杜绝运行时栈溢出；
- [ ] **作用域与闭包 Upvalue 编译加固**：
  - 进一步完善深层块级作用域遮蔽日志（`scope_shadow_log`）；
  - 精准追踪闭包逃逸捕获，避免多余槽位分配；
- [ ] **动态编译接口与作用域降级 (eval & Function)**：
  - 导出供运行时按需调用的轻量级动态编译入口（`compile_eval_code` 与 `compile_function_body`）；
  - 针对含直接 `eval` 的函数作用域实行词法槽位降级保护，防止局部变量优化丢失；
  - 发射合规独立的 `FunctionTemplate`，确保动态字节码 100% 通过 Verifier 静态校验。

### 3. CLI 工具与构建流水线 (`alukac`)
- [ ] **`alukac build` 现代包依赖解析**：
  - 严格遵守 `package.json` 的 `exports` 与 `imports` 规范；
  - 正确解析子路径别名与条件导出；
- [ ] **错误诊断输出优化**：带代码行号、列号与波浪线高亮的友好 SyntaxError / CompileError 诊断提示。
