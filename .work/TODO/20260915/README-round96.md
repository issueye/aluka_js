# 2026-09-15 · 续轮 TODO（M7.2 轮九十六：子目录入口镜像 root + `aluka test` 构建路径 + `assert` 深比较面）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round95.md](./README-round95.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS（本机 v22.3.0）

**本轮范围**：轮九十五 §6 登记的遗留项，按影响面排序推进：
① 子目录入口的跨目录 `require`（**已修**）；② `aluka test` 的依赖构建路径（**已修**）；
③ `node:assert` / `node:assert/strict` 深比较方法面（**已修**，由 ② 的实测暴露）。

---

## 1. 待办与结果

| # | 待办 | 结果 |
|---|---|:---:|
| 1 | 子目录入口（`bin/`、`scripts/`、`tools/`）的跨目录 `require` 运行期可解析 | `[x]` §2.1 |
| 2 | `aluka test` 与 `aluka run` 同款装配（有 `node_modules` 时先构建镜像） | `[x]` §2.2 |
| 3 | `node:assert` 补 `deepEqual`/`deepStrictEqual`/`notEqual`/`fail` 等 | `[x]` §2.3 |
| 4 | `node:assert/strict` 补同上深比较面（`equal`≡`strictEqual`、`deepEqual`≡`deepStrictEqual`） | `[x]` §2.3 |
| 5 | 真实项目 `aluka test` 全绿 + e2e 对拍无回归 | `[x]` §3 |
| 6 | 门禁（fmt / clippy / 全量 test） | `[x]` §4 |

---

## 2. 缺陷根因与修复

### 2.1 子目录入口的跨目录 `require`（镜像 root 取错）

**现象**：`aluka run tools/probe-taskboard.js` → `Cannot find module '../src/store'`；
`aluka run tools/cap-probe.js`（自包含）却正常 —— 差异在**跨目录相对依赖**。

**根因**：`run_build` 把镜像 root 取为**入口文件所在目录**，其 `../src/*` 依赖落在 root
之外，被 `rel_from` 退化成扁平的 `_ext/<文件名>`；而运行期**没有 `_ext` 映射**
（全仓 `_ext` 仅出现在编译器 `rel_from` 的生产端）⇒ 必然解析失败。

**修复**（`crates/aluka-compiler/src/build.rs`）：

- 新增 `resolve_build_root`：从入口目录逐级向上取**最近的 `package.json`**（npm 语义的
  项目边界）作为镜像 root；找不到时退回入口目录（保持散脚本场景不变）；
- 新增 `entry_bc_path`：按同一套 root/相对化规则给出入口 `.bc` 路径，供调用方定位；
- `run_build` 改用 `resolve_build_root`。

镜像因此保持**相对结构**：`tools/probe-taskboard.js` → `aluka_build/tools/*.bc`、
`../src/store.js` → `aluka_build/src/store.bc`，相对 require 逐一对应命中。

### 2.2 `aluka test` 未走依赖构建路径

**现象**：`aluka test test/` 报 `Cannot find module '../src/store'`（三个用例文件全失败）——
`test_command` 直接 `runtime.execute_file(file)`，不构建依赖闭包。

**修复**（`crates/aluka-cli/src/main.rs`）：

- 抽出 `build_if_needed`（与 `run_command` 同一判定：项目根有 `node_modules` 才构建），
  `run_command` 与 `test_command` 共用；
- `test_command` 改为「需要时构建 → 执行入口字节码」，否则退回 `execute_file`。

**配套**（`crates/aluka-runtime/src/lib.rs`）：新增 `Runtime::execute_bc_file`——与
`execute_file` 收尾语义等价（测试运行器 `auto_test_run`、LCOV 生成、未捕获异常格式化、
`process.exit` 退码映射），并保留全部执行记录供宿主判定；`bc_entry::execute_bc` 仍是
命令行直执行的单一事实来源（不回退该路径）。

### 2.3 `node:assert` / `node:assert/strict` 缺深比较面

**现象**：修好 2.2 后用例进入断言阶段即报 `deepEqual is not a function`
（`assert.strict.deepEqual(a, b)`——项目单测用 `node:assert/strict`）。

**根因**：两个断言模块方法面过窄（`assert` 仅 `ok/equal/strictEqual/throws`；
`assert/strict` 仅 `ok/equal/strictEqual/notStrictEqual/throws`），且 `equal` 原为
**字符串化比较**（`format_value(a) == format_value(b)`），与 `===` 语义不同。

**修复**（`crates/aluka-vm/src/builtins/{assert,assert_strict}.rs`）：

- 复用既有单实现 `builtins::test::asserts::{strict_equal, loose_equal,
  deep_strict_equal}`（`node:test` 的 `t.assert` 同源），不再自造比较逻辑；
- `assert`：补 `notEqual` / `notStrictEqual` / `deepEqual` / `notDeepEqual` /
  `deepStrictEqual` / `notDeepStrictEqual` / `fail`；`equal` 改走 `loose_equal`
  （`==` 语义）、`strictEqual` 改走 `strict_equal`（`===` 语义）；
- `assert/strict`：补同一集合，并按 Node 语义令 `equal` ≡ `strictEqual`、
  `deepEqual` ≡ `deepStrictEqual`；
- 两个模块的编译期签名锚点测试同步补齐。

> 已知简化（如实登记）：`deepEqual`（宽松深比较）复用 `deep_strict_equal` 实现——
> 叶子值先走 `strict_equal`，覆盖真实项目的断言取值域（原始值/数组/普通对象）；
> `Number` 与数字字符串这类跨类型深比较不在其中。

---

## 3. 达成证据

### 3.1 子目录入口（修复前 → 修复后）

```text
# 修复前
$ aluka run tools/probe-taskboard.js
Cannot find module '../src/store'

# 修复后（镜像保留相对结构）
$ aluka run tools/probe-taskboard.js
conflict: typeof=object name="ConflictError" code="CONFLICT" status=409 message="重复任务（指纹 9db6bad689）: T001"
step: constructing TaskServer
step: constructed, server.server= object
step: calling listen
listen resolved: true
PROBE7_DONE

$ ls aluka_build/**/*.bc
src/errors.bc  src/http-server.bc  src/service.bc  src/store.bc  tools/probe-taskboard.bc
```

### 3.2 `aluka test`（修复前 → 修复后）

```text
# 修复前
$ aluka test test/
Cannot find module '../src/store'      ← 三个用例文件全失败

# 修复后
$ aluka test test/
ℹ tests 5   ℹ pass 5   ℹ fail 0
ℹ tests 11  ℹ pass 11  ℹ fail 0
ℹ tests 6   ℹ pass 6   ℹ fail 0
# 合计 22/22，与 node --test（22/22）完全一致
```

### 3.3 真实项目 e2e 无回归

```text
$ aluka run e2e.js → exit 0，54 行；Compare-Object(node, aluka) → IDENTICAL
$ aluka run tools/cap-probe.js vs node → IDENTICAL
```

---

## 4. 门禁（全绿）

```text
$ cargo fmt --all --check                                    FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings    CLIPPY_EXIT=0
$ cargo test --workspace --all-features --no-fail-fast -- \
    --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
TEST_EXIT=0
suites=92  passed=652  failed=0
test node22_conformance_matches_node_stdout ... ok
test test262_subset_conformance ... ok
```

---

## 5. `git diff` 复审

```text
 crates/aluka-compiler/src/build.rs            | resolve_build_root + entry_bc_path；run_build 用项目根
 crates/aluka-cli/src/main.rs                  | build_if_needed 抽出；run_command/test_command 共用
 crates/aluka-runtime/src/lib.rs               | 新增 Runtime::execute_bc_file（收尾与 execute_file 等价）
 crates/aluka-vm/src/builtins/assert.rs        | 方法面 4 → 11；equal/strictEqual 改走单源比较实现
 crates/aluka-vm/src/builtins/assert_strict.rs | 方法面 5 → 11；深比较等价映射
```

逐块审核：仅含本轮 3 项修复 + 编译期锚点测试同步；无夹带改动、无调试残留。

---

## 6. 仍未修复（登记，承接轮九十五）

| # | 项 | 说明 |
|---|---|---|
| 1 | `Error` 实例缺自有 `stack` 属性 | `Object.getOwnPropertyNames(err)` 少 `stack` |
| 2 | `process.argv` 未透传命令行参数 | `aluka run x.js arg` 的 arg 不达 `process.argv` |
| 3 | 根目录 `aluka.exe` 陈旧 / `--capabilities` 报 `native: 0` | 轮九十三既有登记 |
| 4 | `deepEqual` 宽松语义简化 | 复用 `deep_strict_equal`（跨类型叶子值如 `1` vs `'1'` 未覆盖） |
| 5 | 事件循环真实时钟保真度 | 现为虚拟时钟推进（顺序正确、数值更快） |

