# 2026-09-07 · 每日 TODO（第三轮：M2 模块系统第一批落地）

> 总 TODO 见 [../README.md](../README.md)；证据规则见其 §0。

**当前里程碑**：M2 (模块系统与真实生态承载)　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标（可判定完成态）

1. **M2.1** `package.json` `exports` / `imports` 条件映射（Node PATTERN_KEY_COMPARE 特异性、`null` 阻断、`./*` 通配、require/import/node/default 条件、`@scope/pkg` 拆分）；
2. **M2.3** `import.meta` 元属性（`url` / `filename` / `dirname` / `resolve(specifier)`）：解析器元属性识别、CJS 内联全局解析与 ESM wrapper 双形态注入、`resolve` 模块说明符解析；
3. **M2.2** TLA 现状核验：单模块顶层 await 可用；跨模块 TLA 导出时序缺口记录在案；
4. 门禁全绿并回填证据。

---

## 2. 待办清单（结项状态）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | aluka-module：极小 JSON 解析器（键序保持）+ `resolve_exports`/`resolve_imports`/`split_package_specifier` + 6 项单测 | `[x]` | M2.1 |
| 2 | aluka-vm `require` 运行时解析接入 exports/imports（含 `#alias`、exports 唯一入口面语义） | `[x]` | M2.1 |
| 3 | alukac build 依赖图解析接入 exports/imports（源码候选层面） | `[x]` | M2.1 |
| 4 | `import.meta` 四表面 + 双形态注入 + `importMeta.resolve` 处理器 | `[x]` | M2.3 |
| 5 | TLA 现状核验与缺口登记 | `[x]` | M2.2 |
| 6 | 门禁验证与证据回填 | `[x]` | 门禁 |

---

## 3. 达成目标证据（真实证据闭环）

### M2.1 · exports / imports 条件映射
**结论**：达成　**证据类型**：命令证据 + 产物证据
- `cargo test -p aluka-module` → `9 passed; 0 failed`（精确/条件/通配/特异性/null 阻断/@scope 拆分/imports 别名）；
- 端到端对拍（`node app.js` vs `alukac build app.js + aluvm app.bc` 输出逐字一致）：

```
exports main: hi from coolpkg exports main
util module loaded: true
```
  fixture：`node_modules/coolpkg/package.json` 的 exports 条件映射（`.` require/import 分支 + `./util` 子路径）；
- 产物：`crates/aluka-module/src/lib.rs`、`crates/aluka-vm/src/modules.rs`、`crates/aluka-cli/src/bin/alukac/build.rs`。

### M2.3 · import.meta
**结论**：达成　**证据类型**：命令证据
- `aluka run` 实测（.mjs）：

```
url ends mjs: true        ← import.meta.url 为 file:/// 形态且指向本模块
filename type: string
dirname: string
resolve: function         ← import.meta.resolve(specifier) 解析至绝对路径
resolved ends dep.js: true
```
- 产物：`crates/aluka-parser/src/parser.rs`（`import.meta` 元属性解析）、`crates/aluka-vm/src/modules.rs`（`build_import_meta` + wrapper 注入 + `resolve_module_for_meta`）、`crates/aluka-vm/src/builtins/module.rs`（`importMeta.resolve` 处理器）。

### M2.2 · TLA 现状核验
**结论**：部分达成（缺口登记）　**证据类型**：命令证据
- 单模块顶层 await：`const v = await Promise.resolve(42); console.log(v)` → `42` ✓；
- 跨模块 TLA：`export const value = await ...` 的导入方读取时序早于异步完成 → `undefined`（node 为 7）。**缺口**：需要异步模块加载器 DAG（ awaiting module graph ），登记为 M2.2 后续主任务，不在本轮强推。

### M2.3 · createRequire
**结论**：既有实现核验可用（`module.createRequire` 已在 builtins/module.rs，含 `node:` 前缀与文件模块解析）。

---

## 4. 自动化门禁结果（全绿才可交付）

```bash
cargo fmt --all --check
# FMT-OK
cargo clippy --all-targets --all-features -- -D warnings
# error 计数 = 0
cargo test --workspace --all-features
# passed: 536, failed: 0
```

---

## 5. 复审结论与偏差记录

- **`git diff` 复审**：变更集中在 aluka-module（解析库）、aluka-vm（require/import.meta）、alukac build（依赖图）、aluka-cli Cargo.toml（aluka-module 依赖），与 M2 目标一致；
- **依赖方向修正**：aluka-module 移除对 aluka-vm/aluka-compiler 的未使用反向依赖，改为 aluka-vm/aluka-cli 依赖 aluka-module（纯解析库下沉）；
- **后续主任务**：M2.2 异步模块加载器 DAG（跨模块 TLA）；M2.4 Express 真实依赖树构建与 e2e（需先以 alukac build 预编译 express 依赖树并逐包排障）。
