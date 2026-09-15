# 2026-09-15 · 续轮 TODO（M7.2 轮九十三：`path` 子对象分派断裂修复）

> 总 TODO 见 [../README.md](../README.md)；本轮上一轮见 [./README.md](./README.md)（轮九十二）。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M7（终局合并与全面验收；M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS
（本机实测 `node --version` = **v22.3.0**，与文档 §「v22.23.1+」不一致，见 §5 遗留 6）

**本轮起因**：对 HEAD（`df07ac2`）做只读工程分析时，发现轮九十二新增的
`path.posix` / `path.win32` 子对象**方法面不可调用**（暴露即坏），并伴有两项
同源缺口（`path.toNamespacedPath` 同缺陷类；5 例已隔离 path 用例其实已转绿）。

---

## 1. 本轮目标（可判定完成态）

1. **`path.posix.<m>()` / `path.win32.<m>()` 直调**：从「抛 TypeError」推进到
   与 Node 逐例一致（8 个方法 × 两face）；
2. **方法值提取形态**（`const f = path.posix.join; f(...)`）同样与 Node 一致
   （两条分派链：`CALL_METHOD` 回退 + `invoke_callable` 都按 NativeFn 名查表）；
3. **补差分语料覆盖**：新增 `path-sub` 域（子对象方法调用 / 方法值提取 / 常量面），
   使该缺陷类今后由门禁自动捕获（本轮漏检根因即「语料无 `path.posix` 用例」）；
4. **回收已转绿的隔离用例**：`gen/deviations` 中 5 例 path 用例（join/dirname/
   isAbsolute/normalize/sep）实测已与 Node 一致，回归 `gen/`；
5. 不引入新的枚举面偏差（`Object.keys(path.posix)` 与 Node 的差异不得扩大）；
6. 门禁：`cargo fmt` / `cargo clippy -D warnings` / `cargo test --workspace --all-features`；
7. 真实证据回填本文件 + `git diff` 逐块复审。

---

## 2. 待办清单（开工先登记）

| # | 待办任务项 | 状态 | 关联 |
|---|---|:---:|---|
| 1 | 修复 `path.posix`/`path.win32` 子对象分派键登记（`builtins/mod.rs`） | `[x]` | M7.2 |
| 2 | `tools/gen.mjs` 增 `path-sub` 域（子对象两face × 方法调用/方法值提取/常量） | `[x]` | 门禁覆盖 |
| 3 | 按生成器格式落语料（`gen-path-sub-*.cjs`），字节级校验生成器一致性 | `[x]` | 门禁覆盖 |
| 4 | 回收 5 例已转绿隔离用例（`gen/deviations` → `gen/`） | `[x]` | DEVIATIONS.md |
| 5 | 重建二进制并跑 `path` 相关差分（含提取形态、枚举面） | `[x]` | 验收 |
| 6 | 门禁验证（fmt / clippy / 全量 test） | `[x]` | 门禁 |
| 7 | 证据回填与 `git diff` 复审 | `[x]` | 证据闭环 |

---

## 3. 根因与修复（待回填实测证据）

**根因（代码级）**：`register_all` 为子对象方法分配 `alloc_native_fn("path.posix.<m>")`，
但只登记了平台模块 `path` 的键 `path.<m>`（`register_handler(registry, "path", ...)`）。
引擎有两条**按 NativeFn 名查分派表**的链路：

- `interpreter.rs` CALL_METHOD 普通对象回退：`self.builtin_registry.lookup(name)`；
- `call.rs` `invoke_callable`（提取方法值后调用）：`self.builtin_registry.lookup(name)`。

`"path.posix.<m>"` 无人登记 → 回退链取不到 handler → 落「方法值不可解析为函数」
分支抛 `TypeError: [function Function] is not a function`。
`path/posix`（独立子模块）的键是 `path/posix.<m>`，与子对象**不是同一命名空间**，
不能复用；`try_dispatch` 形态二（`_builtinNs`/模块单例）对子对象也不适用
（子对象既未注册为模块单例、也不应挂 `_builtinNs`——那会成为可枚举自有属性，
扩大 `Object.keys` 偏差，`punycode.ucs2` 已有此 wart，不在本轮扩散）。

**修复**：按子对象命名空间登记分派键（`path.posix.<m>` / `path.win32.<m>`），
与 NativeFn 名严格同形。

```diff
--- a/crates/aluka-vm/src/builtins/mod.rs
+++ b/crates/aluka-vm/src/builtins/mod.rs
@@ -342,9 +342,20 @@ pub fn register_all(vm: &mut Vm) -> Result<(), VmError> {
         ] {
             let sub_obj = vm.alloc_ordinary();
-            for (m, _) in methods {
-                let f = vm.alloc_native_fn(&format!("path.{sub}.{m}"));
+            // 子对象方法的分派键必须与方法值（NativeFn 名）**严格同形**：……
+            let sub_ns = format!("path.{sub}");
+            for (m, handler) in methods {
+                let f = vm.alloc_native_fn(&format!("{sub_ns}.{m}"));
                 let _ = vm.set_property(Value::Object(sub_obj), m, Value::Object(f));
+                register_handler(&mut registry, &sub_ns, m, *handler);
             }
```

**改动范围**：源码 `crates/aluka-vm/src/builtins/mod.rs` +15/−2；生成器
`tests/conformance/node22/tools/gen.mjs` +37；语料 +29（`gen-path-sub-*`）+
5 例回收（`gen/deviations` → `gen/`）。**无夹带改动**。

---

## 4. 达成证据（真实证据闭环）

### 4.1 修复前 / 修复后 差分对拍（同一语料、同一 harness）

语料：新增 `gen-path-sub-*` 29 例（子对象两 face × 方法调用 / 常量 / 方法值提取）。
判定：`alukac` 编译 → `aluvm` 执行 → 与 `node`（v22.3.0）stdout 逐字节对拍。

```text
# 修复前（git checkout 回退补丁后重建，日志 .work/scratch/evidence_before_pathsub.log）
Result: 4/29 passed, 0 invalid
test result: FAILED. 0 passed; 1 failed
# 失败样例（node vs vm）
  node: "a/c"        vm : ERR TypeError       ← path.posix.join("a/b","../c")
  node: "/a/b"       vm : ERR TypeError       ← path.posix.dirname("/a/b/c")
  node: "..\\c"      vm : ERR TypeError       ← path.win32.relative(...)
# 通过的 4 例 = 4 个常量面（posix.sep/posix.delimiter/win32.sep/win32.delimiter）

# 修复后（日志 .work/scratch/evidence_after_pathsub.log）
Result: 29/29 passed, 0 invalid
test result: ok. 1 passed; 0 failed
```

### 4.2 平台 `path` 面回归复验（含回收用例）

```text
$ ALUKA_CONF_FILTER=gen-builtin-path conformance_node22_test
Result: 12/12 passed, 0 invalid      # 原 7 例 + 本轮回收 5 例
```

### 4.3 三面语义与 Node 逐字一致（探针，`.work/scratch/`）

```text
# probe_path_sub_analysis.js（子对象方法调用）
aluka: path.posix.join => "a/c" / basename => "c.js" / win32.join => "a\\c" / sep·delimiter 一致
node : 逐行相同

# probe_dispatch_shape.js（方法值提取形态 = invoke_callable 按名分派）
aluka: extracted posix.join: a/c   call form: a/c
node : 逐行相同

# probe_keys_analysis.js（枚举面：确认未引入新偏差）
aluka: Object.keys(path.posix) = 10 键（8 方法 + sep + delimiter），与修复前**完全一致**
node : 16 键（多 toNamespacedPath/format/parse/_makeLong/win32/posix）
      → 差异为**既有**面缺口，本轮未扩大（见 §5 遗留 1/2/3）
```

### 4.4 语料产物与生成器一致性

```text
$ node .work/scratch/gen_tools/gen.mjs        # 生成器副本（避免其 rmSync 动到真实语料）
generated 1037 cases across 42 domains
# 新增域 path-sub = 29 例，逐文件落盘 tests/conformance/node22/cases/gen/gen-path-sub-0001..0029.cjs
# 回收 5 例（generator-parity=True，即与生成器输出内容一致）：
#   gen-builtin-path-0002/0006/0009/0011/0013.cjs  → gen/（不再隔离）
$ node .work/scratch/gen_tools/gen.mjs        # 生成器为内核一致源
```

### 4.5 门禁

```text
$ cargo fmt --all --check            → FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings   → （见 §4.6 回填）
$ cargo test --workspace --all-features                     → （见 §4.6 回填）
```

### 4.6 门禁与全量回归（真实输出）

```text
# ① 格式门禁
$ cargo fmt --all --check
FMT_EXIT=0

# ② 严格 Clippy（零告警）
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.51s
CLIPPY_EXIT=0            # 日志中 warning 计数 = 0

# ③ 全量工作区测试（--no-fail-fast，见下方「两例跳过说明」）
$ cargo test --workspace --all-features --no-fail-fast -- --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
TESTB_EXIT=0
suites=92  passed=650  failed=0
test node22_conformance_matches_node_stdout ... ok      # Node 22 差分（全量语料）
test test262_subset_conformance ... ok                  # test262 子集
```

**两例跳过说明（环境敏感，非回归）**：首次门禁以「WMI 脱离会话」方式执行
（本环境前台命令有 30s 上限，全量套件无法前台跑完），该方式下子进程**无控制台**，
`tty_surface`（断言 `isatty: false false false false false`）与 `readline_eof_close`
（断言 `answered: false closed: true`）的 stdio 语义随之改变而失败。
判定为环境产物而非回归的依据：

```text
# ① 两例在**前台（有控制台）**单独复验均通过（日志 .work/scratch/evidence_attached_io.log）
$ builtins_phase7_io_test.exe tty_surface_e2e_matches_go readline_eof_close_e2e_matches_go --nocapture
test readline_eof_close_e2e_matches_go ... ok
test tty_surface_e2e_matches_go ... ok
test result: ok. 2 passed; 0 failed

# ② 失败签名（无控制台）与本次改动无代码路径交集：本轮仅新增 `path.posix.*`/
#    `path.win32.*` 分派键登记，未触碰 tty/readline/stdin 任何代码。
# ③ 计数闭合：650 passed + 2 skipped = 652 = 仓库 `#[test]` 总数（实测 652）。
```

### 4.7 差分总例数（M7.2 口径）

```text
# Node 22 全量差分语料（含本轮新增 gen-path-sub 29 例 + 回收 5 例）
$ cargo test -p aluka-cli --all-features --test conformance_node22_test -- --nocapture
[conf] 共 914 例：隔离敏感 22 例串行 / 纯语义 892 例并行（jobs=8）
Result: 909/909 passed, 5 invalid
test result: ok. 1 passed; 0 failed        finished in 51.17s
CONF_EXIT=0
# 5 invalid = node 侧自身失败（无效对比，不计失败，M1 防假阳性口径）

# test262 子集（含 1000 例官方导入 m72-*）
$ cargo test -p aluka-cli --all-features --test test262_subset_test -- --nocapture
test262 subset: 1130/1154 passed（24 invalid）
test result: ok. 1 passed; 0 failed        finished in 58.39s
T262_EXIT=0
```

**执行记录（如实登记，避免误读为回归）**：本取证脚本**首跑**因漏写
`--all-features` 触发编译期错误（`aluka-runtime` 是 `aluka-cli` 的**可选依赖**，
由 `runtime` feature 启用 → `aluvm.rs` 报 `E0433 cannot find crate aluka_runtime`），
`CONF_EXIT=101`；修正命令后 `CONF_EXIT=0`、`909/909`。**与代码改动无关**。

---

## 5. 登记后续项（本轮不做）

| # | 项 | 现状 | 说明 |
|---|---|---|---|
| 1 | `path.toNamespacedPath` 同缺陷类 | 暴露即坏 | 平台 `path` 上创建了 NativeFn（`interpreter.rs`），但无 handler，也无实现 —— 调用抛 `[function Function] is not a function`（Node 返回 `\\?\C:\x`）。需按 Node `win32.toNamespacedPath` 实现 + 登记键；posix face 为原样返回 |
| 2 | `path.parse` / `path.format` / `path._makeLong` 缺失 | 面缺失 | Node 三面均有；aluka 三面全无（`parse is not a function`）。轮九十二替换平台 path 实现时丢掉了旧轻量实现的 parse/format。隔离用例 `gen-builtin-path-0010`（`path.parse(...).ext`）即此缺口 |
| 3 | `path` 面方法集不全 | 面缺失 | 子对象仅 8 方法 + sep/delimiter；Node 另有 `toNamespacedPath`/`format`/`parse`/`_makeLong` 与自嵌套 `win32`/`posix` |
| 4 | `partition.py` 工具缺失 | 工具缺失 | `gen/DEVIATIONS.md` 头注明「partition.py 自动产出」，但 `tools/` 下只有 `gen.mjs`；隔离/回归只能手工搬迁（本轮即手工回收 5 例） |
| 5 | `gen.mjs` 重跑会复原隔离用例 | 工具陷阱 | 生成器 `rmSync` 全部 `gen-*.cjs` 后重写全集，而隔离用例仍在表达式表中 → 直接重跑会把 172 例偏差重新灌回活跃语料。建议生成器跳过隔离清单 |
| 6 | Oracle 版本口径 | 不一致 | 文档要求 Node.js 22 LTS **v22.23.1+**；本机为 **v22.3.0**。差分结论受小版本影响，建议固定 oracle（`.nvmrc` + CI 固定版本） |
| 7 | 根目录 `aluka.exe` 陈旧 | 交付物过期 | 该文件最后一次提交于 `dd63e39`（M7.1），行为与 HEAD 不一致（`path.join('a/b','../c')` 返回 `a\b\..\c`、无 `normalize`、`path.posix === undefined`）。M7.1「单文件分发」验收若以它为据即不可信 |
| 8 | `--capabilities` 与实际不符 | 诊断失真 | 报 `native: 0 / planned: 58`，而引擎实际注册 **65** 个内置模块（`builtin_modules!()`）；`aluka-builtins::PLANNED_MODULES` 实现完成后从未回写 |
