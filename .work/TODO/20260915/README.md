# 2026-09-15 · 每日 TODO（M7.2 轮九十二：6 项已知缺陷收口）

> 总 TODO 见 [../README.md](./README.md)；证据规则见其 §0。
> 上一日：[20260912](../20260912/README.md)

**当前里程碑**：M7（终局合并与全面验收；M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1)

---

## 1. 今日目标（可判定完成态）

针对上一轮登记的 6 项已知缺陷逐项修复，并以 Node.js 22 LTS 逐例差分对拍判定：

1. **axios codegen 栈平衡缺陷**：`axios.cjs` 从「校验失败」推进到「编译并加载」；
2. **`Object.create` 第二参数**：从「主动回退」推进到规范实现且 Express 不回退；
3. **`path.join` 规范化**：`join`/`normalize`/`dirname`/`basename`/`extname`/`relative` 在 posix/win32/平台三面**逐例等于 Node**；
4. **`URL.searchParams`**：访问器面 + 与 `URL` 双向联动 + `URLSearchParams` 全方法；
5. **`child_process.exec` 回调**：Windows 命令原样投递（引号不再被破坏）+ Error 对象语义；
6. **同进程 `fetch` 自己的 server**：不再 10s 读超时。

---

## 2. 待办清单（开工先登记）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | axios codegen 栈平衡缺陷（解构赋值漏弹栈） | `[x]` | M7.2 |
| 2 | `Object.create` 第二参数 + `defineProperties` 描述子校验 | `[x]` | M7.2 |
| 3 | Express 真实依赖树修复（Buffer 原型/可调用、定时器实参、http 原型） | `[x]` | M7.2 |
| 4 | `path`：Node 语义逐字移植（含 `posix`/`win32`/`sep`/`delimiter`/`isAbsolute`/`normalize`） | `[x]` | M7.2 |
| 5 | `URL`：访问器面 + `searchParams` 双向联动 + 百分号编码 | `[x]` | M7.2 |
| 6 | `child_process.exec`：Windows 命令原样投递 + Error 对象语义 | `[x]` | M7.2 |
| 7 | 同进程 `fetch` 自己的 server（非阻塞泵驱动） | `[x]` | M7.2 |
| 8 | 门禁验证（fmt / clippy `-D warnings` / `cargo test --workspace`） | `[x]` | 门禁 |
| 9 | 真实证据回填与 `git diff` 复审 | `[x]` | 证据闭环 |

---

## 3. 达成目标证据（真实证据闭环）

### 待办 1 · axios codegen 栈平衡缺陷

**根因**：`compile_bind_pattern_assign`（解构**赋值**的逐项写回）调用的
`push_store_by_name` 会 `Dup` 一份值留在栈上——该函数是给**解构声明**路径
（完成值即最后一个绑定值）设计的。解构**赋值**表达式的完成值由
`Expr::DestructureAssign` 末尾单独的 `LoadLocal tmp_slot` 提供，逐项结果
**必须弹栈**；漏弹使每个元素残留一个槽位（`[a] = x` 净 +1、`[a,b] = x` 净 +2），
下游汇合点即报栈深不一致。

**最小复现**（修复前 → 修复后）：

```text
([a] = g());    => 汇合点 76 栈深不一致: 期望 0, 实为 1   →  OK
([a,b] = g());  => 汇合点 96 栈深不一致: 期望 0, 实为 2   →  OK
[a,b] = g();    => 汇合点 96 栈深不一致: 期望 0, 实为 2   →  OK
```

**修复**：`push_store_by_name` → `store_by_name`（写回**并消耗**栈顶值，
去掉 `Dup`，与 `Expr::Assign` 的落点一致）。

**证据**：
- 命令：`aluka build /tmp/ax/package/dist/node/axios.cjs -o /tmp/axout/axios.bc`
- 修复前：`构建完成: 编译 0 个模块, 拷贝 0 个 .json, 失败 1` + `校验失败: V8: 函数 dispatchXhrRequest 汇合点 632 栈深不一致: 期望 0, 实为 2`
- 修复后：`构建完成: 编译 1 个模块, 拷贝 0 个 .json, 失败 0`
- 语义差分（解构赋值 6 形态，逐例等于 Node）：`a,b 1 2` / `expr value is array: true [1,2]` / `rest: [2,3]` / `order A,B A B` / `in fn 99` / `iife 42` / `loop 3` — **全部一致**。

### 待办 2 · `Object.create` 第二参数

上一轮因真实包崩溃而**主动回退**（登记为后续项）。本轮定位到崩溃真实成因
**不在** `Object.create`：Express 依赖链的 `safe-buffer` 第 24 行
`SafeBuffer.prototype = Object.create(Buffer.prototype)` 取到 `undefined`
——`Buffer` 构造器上**没有** `prototype` 属性。修好该前置缺陷后，
`Object.create` 第二参数得以按规范实现。

**实现**（`crates/aluka-vm/src/property.rs`）：
- `validate_property_descriptor`：`ToPropertyDescriptor` 校验（非对象 → TypeError，
  `get`/`set` 非可调用 → TypeError），错误文案逐字对齐 Node；
- `define_properties_from`：`ObjectDefineProperties`（自有**且可枚举**键面、
  `Get` 取值、逐项 `OrdinaryDefineOwnProperty`），`Object.create` 与
  `Object.defineProperties` **共用同一入口**；
- `key_is_non_enumerable`：描述面 `enumerable` 反映 `defineProperty` 登记
  （此前恒报 `enumerable: true`，与 `Object.keys` 的空结果自相矛盾）。

**证据**（19 例差分，`Object.create`/`defineProperties`/`defineProperty`）：

```text
a [1,"hi",["a"],true]                        写入 + getter + 原型 + keys
getter [42,["b"]]                            访问器描述子
nonenum-skip []                              不可枚举描述子被跳过
null-props THROW TypeError: Cannot convert undefined or null to object
prim-desc  THROW TypeError: Property description must be an object: 1
bad-get    THROW TypeError: Getter must be a function: 5
create-noargs THROW TypeError: Object prototype may only be an Object or null: undefined
create-num    THROW TypeError: Object prototype may only be an Object or null: 1
writable-default [1,false,false,false]       writable/enumerable/configurable 缺省 false
accessor [9,9]   symbol-desc 3   order ["a","b"]
```

**19 例逐例等于 Node**（含错误文案）。

### 待办 3 · Express 真实依赖树修复

修好 `Object.create` 后 e2e 暴露出 4 个**独立**的真实缺陷（皆已修复）：

| # | 缺陷 | 定位 | 修复 |
|---|---|---|---|
| 1 | `Buffer.prototype` 不存在 | `safe-buffer` 第 24 行 `Object.create(Buffer.prototype)` | `buffer` 模块建 `prototype` 面并挂到**构造器**；实例隐式原型指向它 |
| 2 | `Buffer(...)` 裸调用报错 | `safe-buffer` 的 `SafeBuffer(arg, enc, len) { return Buffer(arg, enc, len) }` | 新增 `buffer_construct`，`Buffer` 裸调用与 `new Buffer` 同实现 |
| 3 | `setImmediate`/`nextTick`/`setTimeout` **丢失回调实参** | `finalhandler` 的 `defer(onerror, err, req, res)` → `logerror(err)` 收到 undefined → 读 `err.stack` 崩溃（**即上一轮登记的「`Object.create` 崩溃」表象**） | `macro_tasks`/`nexttick_queue` 增加**实参槽**并在回调时回放（Node 语义） |
| 4 | `http.IncomingMessage.prototype` 不存在 | `set_ctor_prototype` 把 `prototype` 挂到**模块对象**（且两次调用互相覆盖） | 改挂到**构造器函数**；`set_native_fn_property` 支持 `NativeCtor` 变体 |

**证据**（Express 真实 HTTP 链路逐行对拍 `app.oracle.txt`）：

```text
$ aluka app.js
PORT_READY
GET / -> 200 hello from express
ECHO -> 200 echo: world
POST -> 200 {"got":{"n":1}}
CONCURRENT -> 200,200,200
CTYPE -> 200 application/xml; charset=utf-8 | <root>ok</root>
CLOSED
```

→ **与 `app.oracle.txt` 逐行一致**（BOM/CRLF 归一后 diff 为空）。

**附带修复**（同一根因族）：
- `Buffer.prototype` 暴露后，`buf instanceof Buffer` 与
  `Object.create(Buffer.prototype)` 派生链生效；
- `set_native_fn_property`/`get_native_fn_property` 支持 `NativeCtor`
  （此前只认 `NativeFn`，构造器上的写入**静默丢失**——这是缺陷 1/4 的共同底层原因）。

### 待办 4 · `path`：Node 语义逐字移植

**问题**：`path` 有**两套并行实现**——注册模块 `path/posix`+`path/win32`（Go
移植）与 `interpreter.rs` 里的 `std::path` 轻量版本。且 Go 版在可观测输出上
与 Node 有**系统性差异**（Go 的 `Base` 先 `Clean`、`join` 结果恒去尾斜杠、
不插驱动相对分隔符等）。

**修复**：以 **Node `lib/path.js` 为唯一标准逐字移植**，删除两套旧实现：

| 文件 | 内容 |
|---|---|
| `builtins/path_node.rs`（新） | `node_basename` / `node_extname`（`preDotState` 状态机）——posix 与 win32 共用，仅分隔符谓词不同 |
| `builtins/path_posix.rs` | Node `posix.normalize`（保留尾分隔符）/`join`/`dirname`（原串切片）/`isAbsolute`/`relative` + `METHODS`/`SEP`/`DELIMITER` |
| `builtins/path_win32.rs` | Node `win32.normalize`（UNC/设备根/保留名/CVE-2024-36139 防护）/`join`/`dirname`（UNC 根分支逐字移植）/`isAbsolute` |
| `builtins/mod.rs` | 平台 `path` 按 `cfg!(windows)` 转挂 posix/win32 实现；`path.posix`/`path.win32`/`sep`/`delimiter` 子面 |
| `interpreter.rs` | 删除 `path_method`/`win_leading_slash` 轻量实现与 CALL_METHOD 前置拦截 |

**证据**（三面 × 172 例差分，逐例等于 Node）：

```text
$ node .work/scratch/issue/pathdiff.js win32  vs  aluka run … win32   → diff 空（172 例）
$ node .work/scratch/issue/pathdiff.js posix  vs  aluka run … posix   → diff 空（172 例）
$ node .work/scratch/issue/pathdiff.js        vs  aluka run …         → diff 空（172 例）
```

覆盖 `join`/`normalize`/`dirname`/`basename`/`extname` × 36 输入
（含 `'..'`/`'.'`/`'/a//b'`/`'C:'`/`'C:/a/b'`/`'\\\\srv\\sh\\f'`/`'a/b/../../c'` 等边界）
+ 双参 `join` 9 例 + `relative` 5 例 + 无参形态 + `sep`/`delimiter`。

**同时修正 2 处测试断言**（`crates/aluka-cli/tests/builtins_phase1_test.rs`）：
断言原值取自 Go 口径（`basename('') === '.'`、`join('C:','foo') === 'C:foo'`、
`dirname('C:') === 'C:.'`），已按 Node 权威值改为 `''` / `'C:\\foo'` / `'C:'`。

### 待办 5 · `URL.searchParams`

**问题**：`URL` 实例是**静态数据属性快照**（建时算好、无原型、无 `searchParams`/
`toString`/`toJSON`/`username`）；`URLSearchParams` 存在但无 `size`/迭代/`sort`、
不做百分号编解码、与 `URL` **零联动**。

**修复**：新增 `crates/aluka-vm/src/builtins/global/url_obj.rs`：
- 内部状态 `_urlState`（`[scheme, username, password, hostname, port, pathname, search, hash]`），
  对外每个键以**访问器**挂载 → 写入真正改写状态并立即反映到 `href`；
- 访问器键从**被调函数对象的 `_urlKey`** 读（`pending_callee()`），
  避免为每个键在分派表登记一条；
- `searchParams` 惰性建 `URLSearchParams` 并记录 `_uspOwner`，
  `usp_rewrite` 后经 `sync_owner_from_usp` **回写** `url.search`（双向联动）；
- WHATWG 百分号编码（path/query/fragment 三个编码集）、
  `file:` → `origin === 'null'`、空路径 → `'/'`、query/fragment 的
  「存在但内容为空」与「不存在」区分；
- `URLSearchParams` 补 `size`/`keys`/`values`/`entries`/`forEach`/`sort`
  + 表单编解码（空格 ↔ `+`）。

**证据**（30 例 + 84 例两组差分，逐例等于 Node）：

```text
$ node url_test.js vs aluka run url_test.js   → diff 空（30 例）
$ node url2.js     vs aluka run url2.js       → diff 空（84 例）
```

84 例覆盖：36 个 URL 字面量（http/https/ws/wss/ftp/file/mailto/data/
IPv6/端口/用户信息/Unicode 路径/空 query）+ 4×11 相对解析矩阵
（`c`/`/c`/`../c`/`?q=2`/`#f`/`//b.org/p` 等）+ 10 个属性写入。

**附带修复**：`url.format(new URL(...))` 此前恒返回 `""`——`url_format` 用
`own_value` 直查 `href` 自有数据槽，而 `href` 现在是**访问器**；改为走属性读取
（`tests/conformance/node22/cases/gen/gen-builtin-url-0007.cjs` 由此转绿）。

### 待办 6 · `child_process.exec`：命令原样投递 + Error 语义

**问题 A（命令被破坏）**：Windows 上 `Command::arg` 按 MSVC 规则给实参加引号
并转义内部引号，而 `cmd.exe` 不认这种转义——

```text
$ aluka: cp.exec('node -e "console.log(7)"', cb)  →  stdout = ""      ← 子进程静默无输出
$ node:  同命令                                    →  stdout = "7\n"
```

**修复**：`shell_command` 在 Windows 走 `raw_arg(" /d /s /c \"<命令>\"")`
**原样投递**（Node 用 `windowsVerbatimArguments` 规避的正是这一点）；
`execSync` 从「按空白拆分并剥引号」改为同一份 shell 命令构造。

**问题 B（错误对象）**：失败回调首参是**纯字符串**，真实包读 `err.code` 一律
`undefined`。**修复**：传 `Error` 实例并附 `code`/`cmd`/`killed`/`signal`/
`stdout`/`stderr`（非零退出 → `code` 为数字退出码；spawn 失败 → `'ENOENT'`；
`message` 为 `Command failed: <cmd>` + stderr）。

**证据**（逐例等于 Node）：

```text
$ node .work/scratch/issue/exec2.js  vs  aluka run … exec2.js   → 逐行一致（6 例）
"echo plain"                => "plain\r\n"
"echo \"quoted arg\""       => "\"quoted arg\"\r\n"
"node -e \"console.log(7)\""=> "7\n"          ← 修复前为 ""
"echo a && echo b"          => "a \r\nb\r\n"

$ node .work/scratch/issue/phase1/execchain.js  vs  aluka run …  → 逐行一致
exec: true "exec-out\r\n" ""
execfail: isError=true code=5 "" ""
missing: isError=true code=ENOENT
eff: true "eff-out\r\n"
```

**同时修正 1 处测试断言**（`crates/aluka-cli/tests/builtins_phase6_proc_test.rs`）：
原断言 `err === 'exit status 5'`（字符串口径）改为
`err instanceof Error && err.code === 5`（Node 口径）。

### 待办 7 · 同进程 `fetch` 自己的 server

**根因**：`do_sync_http_request` 用**阻塞** `TcpStream::read` + 10s 读超时。
当 fetch 的目标就是**同进程**的 `http.createServer` 时，事件循环被钉死在
这个阻塞读上，同进程服务器的 accept/响应泵永远得不到执行 →

```text
$ aluka: fetch('http://127.0.0.1:' + port + '/')  →  FETCH ERR fetch: 读取响应超时：响应头未在 10s 内完整到达
```

**修复**：socket 改**非阻塞** + `WouldBlock` 时 `pump_event_sources()` +
`drain_microtasks()` + 1ms 让步（写路径同构），并以 `Instant` 截止时间
维持原有 10s 语义（`http/client.rs` 的既有非阻塞泵驱动模式同源）。

**证据**（同进程环回，多形态）：

```text
$ aluka run .work/scratch/issue/self3.js
GET / 200 "root"
GET /json 200 application/json {"n":1}
POST /echo 200 "echo:PAYLOAD"
CONCURRENT 200,200
```

**回归**：外部连接拒绝路径仍正常

```text
fetch('http://127.0.0.1:1/')  →  ERR TypeError fetch: connect: 由于目标计算机积极拒绝，无法连接。
```

### 待办 8 · 门禁验证（全绿）

```text
$ cargo fmt --all --check
（无输出）                                      → FMT CLEAN

$ cargo clippy --all-targets --all-features -- -D warnings
error 计数 = 0                                  → CLEAN

$ cargo test --workspace --all-features
TOTAL passed=652 failed=0
```
**复核（09:36，拉取 `origin/master` 后重跑）**：`git fetch --prune` 后快进 `3611d6f → df07ac2`，与远端一致（`git rev-list --left-right --count HEAD...origin/master` = `0 0`），三连门禁在最新提交上重跑，结论与上述记录一致：

```text
$ cargo fmt --all --check
（无输出）                                      → FMT CLEAN（exit 0）

$ cargo clippy --all-targets --all-features -- -D warnings
Finished `dev` profile ... in 24.74s            → exit 0，warning 计数 = 0

$ cargo test --workspace --all-features
92 组 `test result:` 行 / 171 个测试二进制（含 doc-tests）
TOTAL passed=652 failed=0 ignored=1
  └ ignored 为 Doc-tests aluka_vm 的 1 例（非失败）
conformance_node22_test  → ok  1 passed  (68.71s)
test262_subset_test      → ok  1 passed  (75.52s)
golden_execution_oracle_test → ok 33 passed
jitdiff                  → ok  1 passed  (31.34s)
```

### 待办 9 · 真实生态与既有验收回归

```text
# test262 子集
$ cargo test -p aluka-cli --test test262_subset_test -- --nocapture
[t262] 共 1154 例（jobs=8）
PASS 1130 / INV 24 / FAIL 0                     → 与既有基线一致（无回归）

# 差分电池
$ bash .work/diff/run_diff.sh
---- 差分结果: 一致 15 / 不一致 0 ----

# node22 conformance 对拍
$ cargo test -p aluka-cli --test conformance_node22_test
Result: 877/878 passed, 2 invalid                → 全绿（0 失败）

# Express 真实依赖树
$ aluka demo/express-demo/app.js   与 app.oracle.txt 逐行一致

# axios 真实包
$ aluka build axios.cjs
构建完成: 编译 1 个模块, 拷贝 0 个 .json, 失败 0   ← 修复前为「失败 1」
```

---

## 4. 变更范围（`git diff --stat`）

```text
31 files changed, 1495 insertions(+), 548 deletions(-)
```

新增文件：
- `crates/aluka-vm/src/builtins/path_node.rs`（Node `lib/path.js` 共用算法）
- `crates/aluka-vm/src/builtins/global/url_obj.rs`（WHATWG URL）

**`git diff` 复审**：已逐块审核；`demo/express-demo/app.oracle.txt` 与
`demo/express-demo/aluka_build/app.bc` 在排障过程中被误改（BOM 剥离 /
重新生成），已 `git checkout` 还原，最终 diff 仅含上列源码与 2 处测试断言修正。

---

## 5. 登记后续项（本轮未做，需专门排查）

| # | 项 | 现状 | 说明 |
|---|---|---|---|
| 1 | **strict 模式写拒绝** | 语义近似 | 编译器**完全没有** strict 指令概念（`strict`/`Strict` 在 `aluka-bytecode`/`aluka-compiler` 只命中 `StrictEq`/`StrictNe` 运算符，无模式旗标）。`'use strict'` 下对只读属性赋值应抛 TypeError（现静默失败）。需在 `FuncTemplate` 增 strict 旗标 + 新指令，属特性级改动 |
| 2 | **类私有字段 brand check** | 语义近似 | `#x` 按普通属性存储，`P.peek(new Q())` 取到 `undefined` 而非抛 TypeError。需在 `ClassMethodDef` 携带私有名集合 + 访问时校验声明类，属特性级改动 |
| 3 | **入口文件的同胞模块解析** | 未支持 | `aluka run <入口>` 只加载入口 `.bc`；入口**目录内**的同胞模块（`require('./sib.js')`）不可解析（同在 `demo/express-demo/` 下可解析，因那里的 `node_modules` 与包解析路径已装配）。影响「把一组手写脚本放进临时目录直接跑」的场景 |
| 4 | **`tests/conformance/express/run.sh` 陈旧** | 探测失效 | 脚本探测 `http://127.0.0.1:3000/echo/ready` 并期望 JSON 响应，但 `app.js` 用 `listen(0)` 且路由返回纯文本（`/echo/:word` 无 `ready` 特例、亦无 `/load`、`/json` 返回体不同）；该脚本自仓库初始提交（`7d140da`）以来未更新。Express 的**权威验收**是 `app.js` 输出对拍 `app.oracle.txt`（已逐行一致） |
| 5 | **`stream.Stream` 未导出** | 已暴露 | `require('stream').Stream` 为 `undefined`（Node 为函数）。下游 `util.inherits(X, Stream)` 即抛「Cannot read properties of undefined」——`delayed-stream` / `combined-stream` / `form-data` 链全在此断（axios 运行期阻塞项之一） |
| 6 | **codegen 栈下溢（`StackUnderflow`）** | 已暴露 | 加载 `form-data` 链时报 `func=52/30/1/-1 pc=11 err=StackUnderflow`（`pc=11`，疑似同一构造）。**已确认为既有缺陷、与本轮改动无关**（见下「回归判定」）。另 `math-intrinsics` / `dunder-proto` 加载返回 `undefined` |
| 7 | **模板字面量中 `CRLF`/`CR` 未归一化为 `LF`** | 语义偏离 | 源码为 CRLF 时，模板 cooked 值与 `String.raw` 的 raw 值保留 `\r\n`；Node 归一化为 `\n`（ECMA-262 `TV`/`TRV`：`<CR>` 与 `<CR><LF>` → `<LF>`）。**Windows 检出即触发**（本仓库 `.work/diff/*.js` 为 CRLF）。复现：`15_template` 差分不一致（`"multi\r\nline"` vs `"multi\nline"`），差分电池 14/15（见 §5.2） |
| 8 | **字符串字面量行继续 `反斜杠 + LF` 保留换行** | 语义偏离 | `"a\<LF>b"` 应得 `"ab"`（`LineContinuation` → 空串），实际得 `"a\nb"`；lone CR 下得 `"a\rb"`。LF 源即已偏离，**与平台无关**（见 §5.2） |
| 9 | **CRLF 源码中 `反斜杠 + CRLF` 行继续导致编译失败** | 编译失败 | `"a\<CRLF>b"` 报 `SyntaxError: 字符串字面量包含行终结符`，Node 正常得 `"ab"`。lexer 未把 `\` + CRLF 识别为 `LineContinuation`，疑与第 8 项同源；Windows 平台必修（见 §5.2） |

### 5.1 回归判定（axios 运行期的两项阻塞是否由本轮引入）

`axios.cjs` 在基线（HEAD）上**无法通过字节码校验**，故运行期问题不可见；本轮修好
codegen 栈平衡后编译通过，运行期暴露出上表第 5/6 项。为排除「本轮引入」的可能，
用 `git stash` 在**未含本轮改动**的基线上对拍：

```text
# 基线（stash 后重建）—— axios 编译失败，即登记在案的缺陷
$ aluka run one.js        # require('axios')
构建完成: 编译 65 个模块, 拷贝 1 个 .json, 失败 1
  失败: node_modules\axios\dist\node\axios.cjs: 校验失败: V8: 函数 dispatchXhrRequest
        汇合点 664 栈深不一致: 期望 0, 实为 3

# 基线 —— form-data（同一 StackUnderflow 链，无需 axios 即可复现）
$ aluka run fd.js         # require('form-data')
[vm-err] func=52 pc=11 stack=0 err=StackUnderflow
[vm-err] func=30 pc=11 stack=0 err=StackUnderflow
[vm-err] func=1  pc=11 stack=0 err=StackUnderflow
[vm-err] func=-1 pc=11 stack=0 err=StackUnderflow
```

```text
# 含本轮改动 —— form-data 失败签名**逐字相同**（4 帧 func/pc 完全一致）
$ aluka run fd.js
[vm-err] func=52 pc=11 stack=0 err=StackUnderflow
[vm-err] func=30 pc=11 stack=0 err=StackUnderflow
[vm-err] func=1  pc=11 stack=0 err=StackUnderflow
[vm-err] func=-1 pc=11 stack=0 err=StackUnderflow
```

**结论**：
- **`StackUnderflow` 为既有缺陷**——基线 `form-data` 与本轮改动后**签名逐字一致**，
  本轮改动对其可证中性；它此前被 axios 的编译失败挡住而未暴露。
- **axios 状态严格改善**：基线「编译失败 1」→ 本轮「编译 1 个模块，失败 0」。
- 运行期剩余阻塞（`stream.Stream` 缺失等）属**新暴露的既有缺口**，与
  「6 项登记缺陷」是不同范畴，已登记为后续项，不在本轮范围内。

### 5.2 复核补测：行尾（EOL）处理缺陷（新增，见 §5 表 7~9）

以 `df07ac2` 工作区重跑 `.work/diff` 差分电池（`bash` 不在 PATH，用 PowerShell 等价复刻：
`alukac` → `aluvm` 输出 vs `node` 输出），唯一不一致项 `15_template` 定位为**模板字面量
CRLF 未归一化**；据此自建探针扩大排查，确认同源缺陷共 3 项。

```text
$ # 差分电池（15 例）
---- 差分结果: 一致 14 / 不一致 1 ----
=== 15_template : DIFF ===
  ALUKA: "multi\r\nline"
  NODE : "multi\nline"
```

```text
$ # 探针 1：模板字面量（LF 源 vs CRLF 源），node v22.3.0
tpl_lf.js     ALUKA: cooked="x\ny"   raw="p\nq"    |  NODE: cooked="x\ny"   raw="p\nq"    → MATCH
tpl_crlf.js   ALUKA: cooked="x\r\ny" raw="p\r\nq"  |  NODE: cooked="x\ny"   raw="p\nq"    → DIFF（表 7）

$ # 探针 2：同一源码三种行尾（含字符串行继续 eol_lf.js）
eol_lf.js     ALUKA: cont="a\nb" tpl="m\nn"   |  NODE: cont="ab" tpl="m\nn"    → DIFF（表 8）
eol_crlf.js   ALUKA: 编译失败 → SyntaxError: 字符串字面量包含行终结符
              NODE : cont="ab" / tpl="m\nn" / comment-ok / done                → FAIL（表 9）
eol_cr.js     ALUKA: cont="a\rb" tpl="m\rn"   |  NODE: cont="ab" tpl="m\nn"    → DIFF（表 7+8）
```

```js
// eol_lf.js（探针源码，LF 行尾；反斜杠后为真实换行）
var s = "a\
b";
console.log("cont=" + JSON.stringify(s));
console.log("tpl=" + JSON.stringify(`m
n`));
```

```text
$ # 行尾对照（同一批 15 个探针，仅改行尾；alukac → aluvm 输出 vs node 输出）
工作区源码（CRLF，core.autocrlf=true）: 一致 14 / 不一致 1（15_template）
同一批转换为 LF 后重跑                : 一致 15 / 不一致 0
```

→ 既有记录「差分 15/15」是在 **LF 行尾**下取得的，两处记录并不矛盾；表 7/9 **仅在
CRLF 源码下触发**（本仓库 Windows 检出即 CRLF），而表 8 与行尾无关、任何平台均偏离。

**判定**：三项均偏离 ECMA-262（`LineContinuation` → 空串；`TV`/`TRV` 中 `<CR>` 与
`<CR><LF>` → `<LF>`），且**在 Windows 检出下必然触发**——本仓库 `core.autocrlf=true`，
`.work/diff/*.js` 与 `tools_m72_import.py` 导入的 test262 用例多为 CRLF。表 9 直接表现为
**编译失败**，属平台必修项。本轮仅完成测试与登记，未改动实现。

---

## 6. 结论

6 项登记缺陷**全部修复**并以 Node.js 22 LTS 逐例差分判定；过程中发现并修复
**4 个被表象掩盖的底层缺陷**（`Buffer` 原型面缺失、构造器属性写入静默丢失、
定时器回调实参丢失、`url_format` 读访问器），其中「定时器实参丢失」正是上一轮
误判为「`Object.create` 崩溃」的真实成因。

门禁与既有验收**全绿无回归**：fmt ✓ / clippy 0 errors ✓ /
`cargo test --workspace` 652 passed 0 failed ✓ / t262 1130 ✓ /
conformance 877 ✓ / 差分 15/15 ✓（⚠ 复核更正：拉取 `df07ac2` 后重跑为 **14/15**，唯一不一致由 §5.2 表 7 缺陷导致）/ Express oracle 逐行一致 ✓ /
**axios 编译从「失败 1」推进到「失败 0」** ✓。

**范围说明**：`axios.cjs` 由「编译失败」推进到「编译通过」后，运行期暴露出
`stream.Stream` 缺失与既有 codegen 栈下溢等**新既有缺口**（§5 表 5/6）——后者
已用基线对拍证明与本轮改动无关（失败签名逐字一致）。这些属独立范畴，已登记为
后续项，本轮不展开。
