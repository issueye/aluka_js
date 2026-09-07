# 2026-09-07 · 每日 TODO（第二轮：M1 里程碑实施结项）

> 总 TODO 见 [../README.md](../README.md)；证据规则见其 §0。

**当前里程碑**：M1 (ECMAScript 核心规范收口)　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 今日目标（可判定完成态）

1. **M1.1** 实现 `Proxy` 构造器（13 种 traps 派发）与 `Reflect` 全部 13 个静态方法；
2. **M1.2** RegExp 引擎支持后行断言 `(?<=)`/`(?<!)`、命名捕获组 `\k<name>`、`$<name>` 替换、数字反向引用、`\b/\B` 词边界；
3. **M1.4** 类型化数组体系：`ArrayBuffer`/`SharedArrayBuffer`/`DataView` + 11 种 TypedArray 构造函数与完整原型方法；
4. **M1.6** `eval`（直接/间接）与 `new Function` 动态求值（运行时编译器 Hook + 动态字节码强制 Verifier 校验）；
5. **M1.5** test262 语料从 8 例扩容至 ≥100 例；
6. 全工作区门禁（fmt / clippy / test）全绿并回填真实证据。

---

## 2. 待办清单（结项状态）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | M1.1 Proxy & Reflect（heap Proxy 变体 + proxy.rs trap 派发 + reflect.rs 13 静态方法） | `[x]` | M1.1 |
| 2 | M1.2 RegExp 后行断言/命名组/反向引用/词边界（aluka-regex parser+engine） | `[x]` | M1.2 |
| 3 | M1.4 TypedArray 体系（heap 三变体 + typed_array.rs + DataView 字节序） | `[x]` | M1.4 |
| 4 | M1.6 eval/new Function（aluka-vm/eval.rs + 编译器 direct-eval 改写与局部名表标记 + 宿主 eval provider 装配） | `[x]` | M1.6 |
| 5 | M1.3 ES2023-24 方法用例固化（已实现能力补语料） | `[x]` | M1.3 |
| 6 | M1.5 test262 语料扩容 8 → 112 例（新增 104 例 m1-* 用例） | `[x]` | M1.5 |
| 7 | 修复预存在缺陷：对象字面量 `get/set` 命名方法误判为访问器；`MemberAssign/IndexAssign` 闭包捕获退化（对上值对象属性赋值静默失效）；`Reflect.apply` 被 fn.apply 通用协议劫持；register_all 覆盖 Reflect/Proxy 注册 | `[x]` | M1 回归 |
| 8 | 补齐 Array.prototype 缺失方法（reverse/every/includes/indexOf/at/concat/fill/copyWithin/flat/flatMap/splice/keys/values/entries/findLast 等） | `[x]` | M1 回归 |
| 9 | 门禁验证与证据回填 | `[x]` | 门禁 |
| 10 | git diff 复审并提交 | `[x]` | 证据闭环 |

---

## 3. 达成目标证据（真实证据闭环）

### M1.1 · Proxy & Reflect
**结论**：达成　**证据类型**：命令证据 + 产物证据
- `cargo test -p aluka-cli --test test262_subset_test` → `1 passed`（含 m1-proxy-001..045 共 45 例，覆盖 13 traps 全部 + revocable + Reflect 13 方法）；
- 产物：`crates/aluka-vm/src/proxy.rs`、`crates/aluka-vm/src/builtins/reflect.rs`、`crates/aluka-vm/src/heap.rs`（`HeapObject::Proxy` 变体）。

### M1.2 · RegExp 进阶语法
**结论**：达成　**证据类型**：命令证据 + 产物证据
- `cargo test -p aluka-regex` → `15 passed; 0 failed`（含 `lookbehind_positive_and_negative`、`backreferences_match_repeated_text`）；
- 产物：`crates/aluka-regex/src/parser.rs`（`Node::Lookbehind`/`Node::Backref`/`Node::WordBoundary`）、`crates/aluka-regex/src/engine.rs`（CPS 后行断言匹配 + 起点升序贪婪语义对齐 V8）。

### M1.3 · ES2023-24 方法
**结论**：达成　**证据类型**：命令证据
- test262 用例 m1-es2024-001..010 全部通过（withResolvers / toSorted / toReversed / toSpliced / with / Object.groupBy / Map.groupBy / isWellFormed / toWellFormed）。

### M1.4 · TypedArray 体系
**结论**：达成　**证据类型**：命令证据 + 产物证据
- test262 用例 m1-typedarray-001..015 全部通过（11 种构造函数 / 跨视图一致性 / Uint8Clamped 钳制 / DataView LE·BE 字节序 / subarray 共享缓冲 / BigInt64·BigUint64）；
- 产物：`crates/aluka-vm/src/typed_array.rs`（`HeapObject::ArrayBuffer/TypedArray/DataView` 三变体 + 方法分派 + 元素编解码）。

### M1.5 · test262 扩容
**结论**：达成　**证据类型**：命令证据 + 产物证据
- `cargo test -p aluka-cli --test test262_subset_test -- --nocapture` → `PASS` 计数 **112**（扩容前 8）；
- 产物：`tests/conformance/test262/cases/m1-*.js`（104 例）+ 原有 8 例。

### M1.6 · eval / new Function
**结论**：达成　**证据类型**：命令证据 + 产物证据
- test262 用例 m1-eval-001..010 全部通过（直接求值词法穿透与写回 / 间接求值全局写入 / new Function 动态模板 / 动态语法错误拒绝 / 空参 undefined）；
- 产物：`crates/aluka-vm/src/eval.rs`（Runtime Compiler Hook + append-only 模块合并 + 动态字节码强制 `verify()`）、`crates/aluka-compiler/src/module.rs`（直接求值局部名表/上值名表降级标记 + `preserve_completion_value` + `implicit_globals`）、`crates/aluka-runtime/src/lib.rs` 与 `aluka-cli/src/bin/aluvm.rs`（宿主 Hook 装配）。

---

## 4. 自动化门禁结果（全绿才可交付）

```bash
# 1. 格式化门禁（退出码 0）
cargo fmt --all --check
# FMT-OK

# 2. 严格 Clippy 静态分析门禁（零警告允许）
cargo clippy --all-targets --all-features -- -D warnings
# error 计数 = 0

# 3. 全工作区全量测试门禁
cargo test --workspace --all-features
# passed: 530, failed: 0（含 test262_subset_conformance：112 用例 100% 通过）
```

---

## 5. 复审结论与偏差记录

- **`git diff` 复审**：逐文件确认变更与 M1 目标一致，无夹带；
- **已知降级（记录在案）**：
  - Proxy 的 [[Invariant]] 深度校验以「全可配置+全可写」平凡满足（本运行时属性模型无 configurability 位存储）；
  - 直接 eval 中 `var` 新声明绑定的调用方可见性依赖 `implicit_globals` 全局落表，块级 let/const TDZ 穿透未支持；
  - `SharedArrayBuffer` 当前为进程内共享（跨 Worker 共享待 M5 worker_threads 接线时打通）；
  - 部分原型方法（如 `ArrayBuffer.prototype.slice` 调用正常）以「按需合成」形态存在，`typeof obj.method` 属性面读取返回 undefined——与 CALL_METHOD 拦截架构相关，记录为后续统一改造项。
