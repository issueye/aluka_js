# 2026-09-15 · 续轮 TODO（M7.2 轮九十四：真实项目实测 + fs 族补全 + 箭头 `this` 修复）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round93.md](./README-round93.md)。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M7（终局合并与全面验收；M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS（本机 v22.3.0）

**本轮起因**：按要求「创建一个真实项目，测试运行时能否正常使用」——
新建 `demo/taskboard-demo`（存储 + 业务 + HTTP API + CLI + node:test 套件 + 真实 npm 依赖
`ms`），在 Node 22 与 Aluka 上逐行对拍。实测**暴露 5 项运行时缺口**（含 2 项核心缺陷），
其中 2 项已在本轮修复并验证，3 项登记为阻塞项。

---

## 1. 本轮目标与结果

| # | 目标 | 结果 |
|---|---|:---:|
| 1 | 建立真实项目并对拍 Node 22 | `[x]` 项目已建（`demo/taskboard-demo`） |
| 2 | 修 `fs` 同步族缺口（真实项目直接阻塞） | `[x]` 5 API 补全 + 错误形状对齐，探针与 Node 逐字节一致 |
| 3 | 修「类方法内箭头函数 `this` 丢失」核心缺陷 | `[x]` 词法捕获修复，3 个探针与 Node 逐字节一致 |
| 4 | 登记跨模块函数表缺陷（顺序相关，阻塞 e2e 收尾） | `[x]` 根因定位到代码行 + 最小复现 |
| 5 | 门禁（fmt / clippy `-D warnings` / 全量 test） | `[x]` 见 §6 |
| 6 | 证据回填与 `git diff` 复审 | `[x]` 见 §2–§6 |

---

## 2. 修复一：`fs` 同步族补全 + Node 风格错误对象

**背景**：项目 `src/store.js` 的原子落盘用 `fs.renameSync`，Aluka 直接
`TypeError: renameSync is not a function`（`e2e.js` 在 `store` 段即崩）。

**实测缺口清单**（`tools/cap-probe.js`，修复前）：

```text
FAIL fs.renameSync      => TypeError: renameSync is not a function
FAIL fs.unlinkSync      => TypeError: unlinkSync is not a function
FAIL fs.copyFileSync    => TypeError: copyFileSync is not a function
FAIL fs.appendFileSync  => TypeError: appendFileSync is not a function
FAIL fs.realpathSync    => TypeError: realpathSync is not a function
# 错误形状：fs 抛的是**裸字符串** → 真实项目的 `err.code === 'ENOENT'` 分支全失效
errshape readFileSync(missing) => name=undefined code=undefined message=undefined
（Node：name=Error code=ENOENT message="ENOENT: no such file or directory, open '<绝对路径>'"）
```

**修复**（`crates/aluka-vm/src/builtins/fs.rs` + `interpreter.rs`）：

1. 新增 `renameSync` / `unlinkSync` / `copyFileSync` / `appendFileSync` / `realpathSync`
   （注册表 handler + 模块对象方法，NativeFn 名与分派键同形）；
2. 新增 Node 风格 SystemError 构造（`fs_error_object` / `fs_error`）：
   `Error` 实例 + `code`/`errno`/`syscall`/`path`(/`dest`) 属性，
   `message` 按 **libuv 文案表**逐字对齐，路径做绝对化 + 词法归一；
3. `rmSync` 由「NotFound 一律吞掉」改为 Node 语义（`force` 才忽略不存在）；
4. 既有 `readFileSync`/`writeFileSync`/`statSync`/`mkdirSync`/`readdirSync` 全部换用新错误形状。

**证据（修复后与 Node 逐行 diff）**：

```text
tools/cap-probe.js  → IDENTICAL   # 5 个新 API 返回值 + 3 条错误码/文案 + 语言/HTTP 探针
```

未知项（如实登记）：`errno` 为占位值 `-4094`（Node 的 errno 是 libuv 平台相关负值），
判定语义请用 `err.code`（已逐字对齐）。

---

## 3. 修复二：类方法内箭头函数的词法 `this`（核心语言缺陷）

**症状**（`tools/probe-this2.js`，修复前）：

```text
① arrowDirect（方法体内箭头直接调用）      aluka: THROW TypeError reading 'v'   node: 42
② arrowInMap（箭头作 Array.map 回调）      aluka: THROW                          node: 42
⑧ arrowInFrom（箭头作 Array.from 映射器）  aluka: THROW                          node: 42
```

**根因**：`Expr::This` 一律编译为 `LoadLocal 0`（当前帧槽 0）。非箭头函数的 `this`
确实是 `locals[0]`，但**箭头函数不绑定自己的 `this`**——其帧内槽 0 并非词法 `this`，
于是类方法内箭头访问 `this` 全部拿到 `undefined`。

**修复**（`crates/aluka-compiler/src/{scope.rs,module.rs,codegen.rs}`）：

- 新增 `THIS_SYM`（与既有 `HOME_OBJECT_SYM` 同构）：非箭头函数把 `THIS_SYM → 0` 登记进
  `symbol_map`（既解析自身 `this`，又把槽 0 暴露给父作用域快照）；
- 箭头函数**不**登记自身，改从父作用域**预捕获**（优先父局部槽 0，其次父已捕获的
  `THIS_SYM` 上值 → 支持箭头套箭头）；
- 新增 `emit_this()` 三态发射（`LoadLocal` / `LoadUpvalue` / 退化 `LoadLocal 0`），
  统一 `Expr::This` 与 `super.x` / `super.m()` 三个发射点。

**证据（修复后与 Node 逐行 diff）**：

```text
tools/probe-this.js    → IDENTICAL   （EventEmitter 子类 + this.server + Promise + close）
tools/probe-this2.js   → IDENTICAL   （①–⑧ 全形态一致）
```

---

## 4. 未修复的阻塞缺陷（真实项目实测暴露）

### 4.1 【严重】跨模块闭包/类调用误执行**另一模块同索引函数体**（顺序相关）

**复现**（`demo/taskboard-demo/pb-c.js` vs `pb-h.js`，同为静态 require，仅顺序不同）：

```text
pb-a.js  require config, logger, errors                    → ok   "ConflictError" "CONFLICT"
pb-b.js  require config, logger, errors, store             → ok   "ConflictError" "CONFLICT"
pb-c.js  require config, logger, errors, store, service    → 抛 TypeError: Cannot read properties of undefined (reading 'status')
pb-h.js  require config, logger, errors, service, store    → ok   "ConflictError" "CONFLICT"   ← 仅顺序不同
```

四者构造同一句 `new ConflictError('pb-msg')`（`errors.js` 的类，三级继承 Error→AppError→ConflictError）。
抛错内容来自**另一模块（service.js）中读 `data.status` 的代码路径**，而非 `ConflictError` 构造器。

**根因（代码级）**：

- `Vm::run_module` / `load_module_for_test` 每次**整体替换** `self.module_functions`
  （`crates/aluka-vm/src/call.rs:1087` / `:1063`）与 `module_classes`；
- `Vm::resolve_callable`（`call.rs:124-137`）对 `HeapObject::Closure { func_idx, .. }`
  按**当前** `module_functions` 解释 `func_idx`；甚至把「堆对象索引落在当前表长度内」
  也当作函数索引（`call.rs:132`，遗留约定）；
- ⇒ **A 模块创建闭包/类 → 之后 B 模块成为当前表 → 再调用 A 的闭包**时，执行的是
  **B 模块 `func_idx` 处的函数体**；是否触发取决于各模块函数数量与索引处的函数形态 ⇒ 顺序敏感。

**影响**：任何多模块真实项目都可能命中（类构造、跨模块回调、`require` 后的延迟调用）。

**两个已实证的表现形态**：

1. **类构造误派发**（顺序相关）：上表 `pb-c`（失败，抓到 `undefined | undefined`）/ `pb-h`（通过）；
2. **HTTP 回调体不执行**（顺序无关）：`demo/taskboard-demo/probe-http.js` 用项目真实
   `TaskServer`，Node 正常（`listen → 200 {"ok":true,…} → close`），Aluka **挂起**。
   逐步插桩实证：

   ```text
   [dbg] listen() entered port=0 host=127.0.0.1      ← listen 体**正确执行**
   [dbg] listen() registered callback                 ← 回调已注册到真实 http server
   （此后无任何输出 → 回调体从未执行；Node 同脚本打印 callback fired → resolve → 200）
   ```

   构造阶段 Node/Aluka **诊断完全相同**（`instanceof TaskServer` / `ctor.name` /
   `proto.ctor.name = EventEmitter` / `this.server` 与 `this.server.listen` 类型一致），
   故非构造问题；重排 require 顺序（`pb-http-order.js`）**不能**消除挂起。

**布局敏感（重要）**：该缺陷是**潜伏型**——`src/store.js` 增加一个辅助函数
（`cloneTask`）后，e2e 的三个错误路径（`duplicate` / `validation` / `get(missing)`）
从「属性全空」变为**与 Node 一致**，即误派发的落点随函数索引偏移而改变。
⇒ 不能用「某次跑通」判定该缺陷不存在；须以 `pb-c.js` 这类最小复现为准。

**修复方向（建议，未实施）**：让闭包与类模板携带**所属模块的表**（如
`HeapObject::Closure { funcs: Rc<Vec<Rc<FuncTemplate>>>, func_idx, .. }`，或给模块分配
`module_id` + VM 内 `tables: Vec<Rc<Vec<Rc<FuncTemplate>>>>`），调用点按携带的表解析；
`module_classes` 同理需按模块寻址；Rust 侧延迟回调（HTTP/timer/事件）尤须走同一解析路径。

### 4.2 【中】入口位于子目录时，跨目录 `require` 运行期不可解析

```text
$ cd demo/taskboard-demo && aluka run tools/probe-taskboard.js      # 入口在 tools/
Cannot find module '../src/store'
# 构建产物：aluka_build/…（入口所在目录成为镜像 root），../src/* 被扁平化到 _ext/store.bc
# 编译器 build.rs §rel_from：root 之外的文件退化为 `_ext/<文件名>`；运行期无 _ext 映射
```

**影响**：`bin/`、`scripts/`、`tools/` 作入口的常见布局；扁平化后同名文件还会相互覆盖。

### 4.3 【低】`process.argv` 未注入命令行参数

```text
$ aluka run probe-bisect2.js 5   → process.argv = ["aluka_build\\probe-bisect2.bc"]
# 期望（Node 语义）：argv[0]=脚本路径，其后为命令行参数
```

### 4.4 【低】`Error` 实例缺自有 `stack` 属性

```text
ownProps:  node=["stack","message","name","code","status"]
           aluka=["message","name","code","status"]
```

### 4.5 既有登记项（本轮复核，未改动）

- 根目录 `aluka.exe` 陈旧（M7.1 交付物，`path.join` 语义与 HEAD 不一致）；
- `--capabilities` 报 `native: 0 / planned: 58`，实际注册 65 个内置模块；
- Oracle 版本口径（文档 v22.23.1+ vs 本机 v22.3.0）；
- `gen.mjs` 重跑会把隔离用例灌回活跃语料；`partition.py` 缺失。

---

## 5. 达成证据

### 5.1 真实项目对拍（`demo/taskboard-demo`）

```text
$ node e2e.js          node-exit=0   54 行确定性输出
$ aluka run e2e.js     aluka-exit=2  32 行
# 逐行 diff：**共同前缀 = 32 行**（config / dependency[真实 npm 包 ms] / store / service 全段
#           与 Node 逐字节一致）；分歧自 `### http` 起（§4.1 第 2 形态：HTTP 回调体不执行）
```

**项目自带测试套件**（Node 为 oracle）：

```text
$ node --test test/store.test.js test/service.test.js test/api.test.js
# tests 22 / pass 22 / fail 0
$ aluka test test/
Cannot find module '../src/store'   ← §4.2（test/ 下的测试文件 require ../src/* 落到 _ext）
```

**过程中修复的项目自身缺陷（Node 判定，非运行时缺陷）**：

1. `TaskStore.get/all/insert/replace/remove` 原为**浅拷贝** → `copy.tags.push(...)`
   会改到内部状态（Node 断言失败）；改为 `cloneTask()` 显示拷贝 `tags` 数组；
2. 三个测试文件共用 `.data-test/` 目录 → node:test 并行子进程互相删除（ENOTEMPTY/EPERM）；
   改为每文件独立目录（`.data-test-store/`、`.data-test-api/`）。

**已验证与 Node 逐行一致的部分**：

- 配置装载（默认值/派生字段/键序）、结构化日志（`with()` 派生与级别过滤）；
- **真实 npm 依赖 `ms`**：`ms(60000)=1m` / `ms("2d")=172800000` / `ms(1500,{long:true})=2 seconds`；
- 存储层真实落盘与重载（`fileBytes>0` / `fileExists` / `reloadSize`）；
- 业务统计（`counts` / `completion=33` / `topTags` 排序）、列表过滤与排序；
- 语言能力：类/继承/`super`/getter、`EventEmitter`、`Array.from/find/splice/sort`、
  `Buffer.concat/byteLength`、`crypto.createHash`、`Date.toISOString`、模板串、`JSON` 缩进；
- HTTP：`createServer` + `listen(0)` + `address()` + 进程内往返 + `close`（`tools/cap-probe.js`）。

### 5.2 探针结果（修复后与 Node 逐行 diff）

```text
tools/cap-probe.js      → IDENTICAL   （fs 族 + crypto + 语言 + EventEmitter + HTTP 往返）
tools/probe-this.js     → IDENTICAL
tools/probe-this2.js    → IDENTICAL
tools/probe-crossmod.js → IDENTICAL   （跨模块自定义 Error 属性穿透）
tools/probe-errchain.js → IDENTICAL   （三级继承链 + 类方法抛错）
tools/probe-errors.js   → 差异仅：Error 实例缺自有 stack（§4.4）
pb-c.js / pb-h.js       → 顺序敏感缺陷复现（§4.1）
```

### 5.3 门禁（最终状态实测）

```text
$ cargo fmt --all --check                                    FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings    CLIPPY_EXIT=0（Finished，0 告警）
$ cargo test --workspace --all-features --no-fail-fast -- \
    --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
TESTC_EXIT=0
suites=92   passed=652   failed=0
（652 = 仓库全部 #[test]；node22 差分与 test262 子集套件均 ok）
```

**过程中的一次既有测试对齐（如实登记）**：首轮全量跑出**唯一失败**
`fs_sync_family_e2e_matches_go`（`sync_builtins_test.rs`）。该测试脚本第 29 行是
「预清理」`fs.rmSync("d1", { recursive: true })`（**未传 `force`**），依赖旧 Go 口径
「NotFound 一律吞掉」；而 **Node 语义下缺省 `force: false` 对不存在路径抛 ENOENT**。
按 AGENTS.md「Node.js 22 为唯一权威 oracle」，已把该行改为
`{ recursive: true, force: true }`（真实的惯用写法，`demo/taskboard-demo` 同款），
期望输出串**保持不变**。改前先用真实 `node v22` 逐字节验证：

```text
$ node probe.js（含 force:true）      →  names: 2 a.txt b.txt a.txt,b.txt / dir: true false 0 /
                                        file: true false 5 / mtime: number true / deleted: false / gone: true
$ aluka run probe.js                  →  逐字节相同
$ node probe.js（无 force，缺失路径） →  throw ENOENT
$ aluka run probe.js（无 force）      →  throw ENOENT（与新语义一致，见 tools/probe-rm.js：5 种选项组合全对齐）
```

### 5.4 差分回归（改动后无回退）

```text
# path 子对象（轮九十三新增语料）
$ ALUKA_CONF_FILTER=path-sub conformance_node22_test   →  Result: 29/29 passed, 0 invalid
# 全量工作区测试内的 node22 差分与 test262 子集
test node22_conformance_matches_node_stdout ... ok
test test262_subset_conformance ... ok
```

### 5.5 `git diff` 复审

```text
 .work/TODO/20260915/README.md                      |  72 ++++-
 crates/aluka-cli/tests/sync_builtins_test.rs       |   9 +-      ← Node 语义对齐（含理由注释）
 crates/aluka-compiler/src/codegen.rs               |  27 +-      ← emit_this 三态发射
 crates/aluka-compiler/src/module.rs                |  28 +-      ← THIS_SYM 登记/预捕获
 crates/aluka-compiler/src/scope.rs                 |   9 +       ← THIS_SYM 常量
 crates/aluka-vm/src/builtins/fs.rs                 | 297 +++++++  ← 5 API + Node 错误对象
 crates/aluka-vm/src/builtins/mod.rs                |  15 +-      ← path 子对象分派键（轮九十三）
 crates/aluka-vm/src/interpreter.rs                 |  11 +-      ← readFileSync/writeFileSync 错误形状
 tests/conformance/node22/tools/gen.mjs             |  37 ++      ← path-sub 域（轮九十三）
 + 语料 29 新增 / 5 回收 / 5 迁移（gen/deviations → gen/）
 + demo/taskboard-demo/（新增真实项目与探针）
```

逐块审核结论：仅含本轮两项修复 + 一轮九十三的 `path` 修复 + 一处既有测试的
Node 语义对齐 + 新增项目/语料；**无夹带改动**。
`errno` 为占位值（§2 末）与 §4 各缺陷均已在文件内如实登记，未隐去。

