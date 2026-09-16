# 2026-09-15 · 续轮 TODO（M7.2 轮一百零一：真实 TS/ESM monorepo 实测 — earendil-works/pi）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round100.md](./README-round100.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载 → M7.3 npm Top 50 无缝运行）　|　**权威 Oracle**：Node.js 22 LTS（实测 v22.3.0）

**本轮范围**：改用**全 ESM + TypeScript monorepo** 驱动——克隆 `earendil-works/pi`
（Pi Agent Harness，11 包 workspace，`"type": "module"`），构建 chord/telemetry/ai/agent/protocol
五个核心包，对 protocol/ai/agent 的确定性 API 做 Node 逐字节对拍；暴露并以 16 项引擎修复闭环。

实测环境：`git clone`（代理）+ `npm install`（代理）+ tsgo 构建；探针置于仓库根
`.work/scratch/pi/aluka-probe.mjs`（19 项断言）。

---

## 1. 待办（开工先登记）

| # | 待办任务项 | 状态 |
|---|---|:---:|
| 1 | 克隆 pi（代理）并调研项目结构/运行方式 | `[x]` |
| 2 | 安装依赖、构建核心包、确定对拍切入点 | `[x]` |
| 3 | aluka vs Node 真实环境对拍取基线 | `[x]` |
| 4 | 定位并修复可闭环缺陷、登记后续项 | `[x]` |
| 5 | 门禁（fmt / clippy / 全量 test）+ 轮 100 探针回归 | `[x]` |
| 6 | 证据回填与 git diff 复审 | `[x]` |

---

## 2. 实测证据（已回填）

**构建期**：aluka 静态扫描 + 编译 pi 依赖图 **696 个模块，失败 0**（含 typebox 全量
ESM、chord、telemetry、ai、agent、protocol 与传递依赖）。基线上限受下述 ESM 缺口阻塞
（轮初连模块图都编译不出）。

**pi 探针（19 项）终态**：`frame.hello`（文本编码 hex）✓、`repair.*`/`content.*`/`uuidv7.*`
（ai 包）✓、`msg.*`（agent harness 消息构造）✓、`proto.invalid.parse`（校验拒绝面）✓
—— 与 Node 逐字节一致，共 **9/19**；其余 10 项受 §5-1 的 ESM 命名空间兑现缺口阻塞
（同一根因族，非独立缺陷）。

**旁证（分项探针）**：chord 命名空间 26 键 ✓（修复前 1）；framing.js 命名导入/类导出 ✓；
`import.meta.url`/`importMeta.dirname` ✓；顶层异常 `TypeError: boom-esm` + exit 1 ✓；
BigInt 位运算族与 Node 逐字节一致（含混用 TypeError 报文）✓；匿名/继承类表达式 ✓。

**轮 100 探针回归**：lodash/zod/chalk/commander 十探针 **PASS=10 FAIL=0**（零退化）。

## 3. 缺陷与修复（已回填，16 项）

| # | 缺陷 | 根因 | 修复 |
|---|---|---|---|
| 1 | `import _ from 'lodash'` 的 `_` 为 undefined | `__aluka_import__` 裸返回 CJS exports，无合成命名空间 | `module_import` 合成 `{default: module.exports, ...标识符形键}`（`__esModule` 目标与 TLA 异步目标不包装） |
| 2 | `import.meta` 在 ESM 入口为 undefined | `invoke_cjs_entry` 传 `Undefined`（require 加载路径却物化） | 入口按 entry 文件/dir 物化 `import.meta` |
| 3 | ESM 顶层异常静默消失（exit 0、无输出） | async wrapper 拒绝的 Promise 无人消费；且 `invoke_cjs_entry` 丢弃 wrapper 返回 | resume/完成值上抛 + 事件循环后置拒绝检查 → `Thrown` 渲染 + exit 1 |
| 4 | `return class { ... }` 解析失败 | 解析器/编译器/AST 全无类表达式 | `Expr::Class` + `class` 表达式臂 + `MakeClass` 延迟装配（`class_backpatches`） |
| 5 | `class extends Base` 中 `Base` 报 ReferenceError | 自由变量收集器不认识 `Expr::Class` | 两个收集器补臂（ident-uses / closure-capturing） |
| 6 | `1n << 41n`、`&｜^ >> ~` 全抛 TypeError | BigInt 位运算缺失（仅 Sub/Mul/Div/Mod/Pow） | `bigdec.rs` 补码位运算族（符号扩展、算术右移、`~`）+ ops/interpreter 分派 |
| 7 | ESM 顶层 `function t(){}` 未绑定 | `compile_esm_stmt` 无函数/类声明臂 | 补 Function 臂 |
| 8 | `export class X` 未导出 | 同上（类声明臂） | 补 Class 臂（复用 `emit_class_expr` + 导出挂载） |
| 9 | `export * from 'x'` 静默忽略 | 未实现 | for-in 合成重导出（键快照，排除 default） |
| 10 | `export { a } from 'x'` 未导出 | Named 臂无 `source` 处理 | 补命名重导出臂（ns 取值逐项挂载） |
| 11 | `TextEncoder.encode` 返回普通数组 | 用 `alloc_array` | 改为真 `Uint8Array`（`instanceof` 品牌检查通过） |
| 12 | `.js` 一律按 CJS 编译 | 只看扩展名 | 按最近 `package.json` 的 `"type": "module"` 判定（Node 语义） |
| 13 | ESM `await import` 挂起后相对解析基准丢失 | require 基准栈在挂起时弹出 | 相对导入源编译期改写为 `__dirname + "/spec"` + 绝对路径解析 |
| 14 | 入口 `__dirname` 指向镜像/空串 | `setup_cjs` 用 .bc 路径且未绝对化 | **双基准分离**：`base_dir`=镜像（解析）/`source_dir`=源码（观察面），入口绝对化 |
| 15 | 裸包子路径 `typebox/value` 解析失败 | `exports` 只按 `require` 条件匹配（ESM-only 包只配 `import`/`default`） | 运行时与构建侧双条件回退（require → import） |
| 16 | 上下文关键字形参等轮 100 遗留项 | — | 见轮 100 §3（本轮回归验证） |

## 4. 门禁（已回填，真实输出）

```
cargo fmt --all --check                       → FMT-OK
cargo clippy --all-targets --all-features
  -- -D warnings                              → 0 warning / 0 error
cargo test --workspace --all-features
  --no-fail-fast（跳过 2 个 tty/readline e2e）  → TEST_EXIT=0，92 套件 test result: ok，0 FAILED
轮 100 十探针回归                                → PASS=10 FAIL=0
```

## 5. 登记后续项（待回填）

1. **[最高优先] 裸说明符 ESM 加载缺口**（本轮已收窄到加载器）：
   - **判别实验（决定性）**：自造裸包 `node_modules/zzbare`（`type: module`，index.js 含
     一个相对导入 + 导出 const/function）。同一文件经 `import 'zzbare'` 得到**空导出**
     包装形态，经 `import './node_modules/zzbare/index.js'` 完全正常——**两次构建产出的
     `index.bc` 字节完全一致**（md5 相同）。即：模块种类判定、编译产物均排除，差异在用
     **说明符形态决定的加载路径**。
   - `ALUKA_REQ_DEBUG` 记录：相对形式在 append `index.bc` 之后继续 append 内层依赖
     `lib.bc`；裸形式**没有**内层 append——即模块 wrapper 未被调用，加载器落到
     「内联形态」回退（直接读 `module.exports`，此时为空对象、无挂起 Promise），
     `module_import` 因而对空对象做 CJS 命名空间包装。
   - 下一个怀疑点：`module_functions` 合并后的 **TemplateIdx 重写**（`rewrite ... op 2
     already >= fn_base 2` 启发式）与 `run_func(main)` 返回闭包的目标函数索引——裸/相对
     两条路径在同一 `fn_base` 下对 `MakeClosure` 操作数的解释是否一致。
   - 影响面：所有经裸说明符引入的 ESM 包（typebox 及其传递的 codec/protocol 链、
     pi 的 @earendil-works/*）：pi 探针 10/19 项受此单项阻塞。
2. `export *` 重导出的键序：本实现排在自有导出之后（Node 按语句序）——键集一致、顺序不同。
3. `__esModule` 在本运行时命名空间中可枚举（Node 命名空间不暴露）。
4. ESM 函数/类声明提升到 import 之前的规范语义（当前按语句位置绑定）。
5. 动态 `import()` 表达式（`import is not defined`）；`export * as ns from`（别名臂未实现）。
6. `crypto.getRandomValues` 对 TypedArray 无填充（uuidv7 随机段全零；仅影响随机性，不影响格式）。
7. 既有（非本轮引入）：aluvm bin 无 `runtime` feature 时 E0433。
