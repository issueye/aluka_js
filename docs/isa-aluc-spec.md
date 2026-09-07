# Aluka ISA 发布契约：`.aluc` / `.alua` 格式规范（ALUKACC1）

> 状态：M4 采纳。本文档是 **ISA 发布契约** 的格式部分——任何符合本规范的
> 前端产出的 `.aluc`，均可在 aluvm 上执行，无需修改后端。
> 兼容窗口与版本递增权限见 `docs/adr/0003-m4-isa-contract-policy.md`。

## 1. `.aluc` —— 编译产物容器（二进制）

`.aluc` 是**自描述容器**：外层为 ALUKACC1 容器头 + 调试段开关；内层
payload 是完整的 ALUKABC1（Version 30）字节码模块（引擎现行 ISA 语义层，
含指令流/常量池/Try 表）。内外分层使「容器演进」与「ISA 语义演进」解耦。

### 容器布局（小端）

| 偏移 | 大小 | 字段 | 说明 |
|---|---|---|---|
| 0 | 8 | 魔数 | ASCII `"ALUKACC1"` |
| 8 | 4 | 容器版本 | le u32，当前 **1**（容器布局变更才递增） |
| 12 | 4 | flags | le u32：bit0 = HAS_DEBUG |
| 16 | 4 | payload_len | le u32，内层 ALUKABC1 模块字节数 |
| 20 | 4 | debug_len | le u32，HAS_DEBUG=0 时为 0 |
| 24 | payload_len | payload | 完整 ALUKABC1 模块（含 8 字节内层魔数） |
| … | debug_len | debug 段 | 见下 |

### 调试段（HAS_DEBUG=1 时）

| 大小 | 字段 |
|---|---|
| 4 + n | 源文件路径（u32 长度 + UTF-8） |
| 4 | 函数调试条目数 N |
| N × (4 + n) | 每函数：u32 长度 + UTF-8 调试名（源级函数名/行号表预留 v2） |

**剥离选项**：`alukac --strip-debug` 产出 flags bit0=0、debug_len=0 的容器
（分发用）；调试段仅存 supplementary 信息（函数名/源路径随 payload 内联，
供运行时栈回溯，不受剥离影响）。

### 内层 payload（ISA 语义层）

即 `aluka-bytecode::serializer` 输出的 ALUKABC1（Version 30）模块字节流：
20 字节容器头（魔数 + ISA 版本 30 + 函数/类计数）+ 函数模板 + 类模板。
**ISA 兼容版本 = 内层 version 字段**；容器版本只管容器布局。

## 2. `.alua` —— 文本汇编格式（可读转储）

`alukac disasm --format=alua` 产出（确定性文本，人类可读/可 diff）：

```text
.module <source-file>
.version 30
.func <name> params=<n> locals=<n> varargs=<0|1> generator=<0|1> async=<0|1>
.const <idx> num <f64>
.const <idx> str <"escaped">
.const <idx> bigint <text>
.const <idx> bool <true|false>
.const <idx> null
  <pc>: <OPNAME> <operand-hex>
  ...
.try start=<pc> catch=<pc> finally=<pc> has_catch=<0|1> has_finally=<0|1> end=<pc> catch_end=<pc> finally_end=<pc>
.endfunc
.class <name> ...
```

行序确定性（函数/常量按索引序）保证 diff 稳定。`.alua` 汇编器（文本→
字节码）不在 M4 范围（登记后续项）。

## 3. 加载

`aluvm run` 按魔数自动嗅探：`ALUKABC1`（Go 互通格式）或 `ALUKACC1`（本
契约格式）皆可直接执行；`alukac` 默认产出 ALUKABC1（与 Go 前端缓存互通），
`--format=aluc` 产出发布容器。
