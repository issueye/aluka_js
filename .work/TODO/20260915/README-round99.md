# 2026-09-15 · 续轮 TODO（M7.2 轮九十九：`Error.captureStackTrace` 形态统一 + `stackTraceLimit` 生效 + 原生构造器自有面）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round98.md](./README-round98.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS（本机 v22.3.0）

**本轮范围**：轮九十八 §6 登记的 2 项（`captureStackTrace` 形态、`stackTraceLimit` 未生效）。
过程中连带修复 **3 个更底层缺陷**（`own_value` 不认原生构造器、`super()` 到内建父类丢实参、
原生构造器自有面完全不可见），均为 Node 逐项对拍暴露。

---

## 1. 待办与结果

| # | 待办 | 结果 |
|---|---|:---:|
| 1 | `Error.captureStackTrace` 写**字符串** stack（非调用点数组） | `[x]` §2.1 |
| 2 | `captureStackTrace` 第二参数 `constructorOpt` 截断语义 | `[x]` §2.1 |
| 3 | `Error.stackTraceLimit` 参与帧数截断（`0` → 仅首行） | `[x]` §2.2 |
| 4 | 【连带】`own_value` 不认 `NativeCtor`/`NativeFn` 自有面 | `[x]` §2.2 |
| 5 | 【连带】`super(args)` 到内建父类（`extends Error`）丢实参 | `[x]` §2.3 |
| 6 | 【连带】原生构造器自有面在 keys/getOwnPropertyNames/for-in 不可见 | `[x]` §2.4 |
| 7 | `Error.prepareStackTrace` 属性存在（默认 undefined、不可枚举） | `[x]` §2.4 |
| 8 | 门禁（fmt / clippy / 全量 test） | `[x]` §4 |

---

## 2. 缺陷根因与修复

### 2.1 `captureStackTrace` 写「调用点数组」而非字符串

**Node 语义**（本机实测锁定）：

```text
const r = Error.captureStackTrace(e)   → 返回 undefined
e.stack 仍是 **string**（typeof "string"，Array.isArray false），首行 "Error: m"
Error.captureStackTrace(this, MyErr)   → stack 中**不出现** MyErr 帧（含其内侧帧一并丢弃）
```

修复前 aluka 在 `target.stack` 上写 12 个 callsite **对象数组**（`Array.isArray(e.stack) === true`），
与 `stack` 的字符串形态冲突，且 `String(err.stack)` 得到 `[object Object]` 形态。

**修复**（`builtins/global/error.rs` + `heap.rs`）：改为委托统一生成器
`Vm::fill_error_stack(target, constructor_opt)`——复用轮九十八的
`build_error_stack_with`（首行 + 调用帧），并支持 `constructorOpt`
（`rposition` 定位该构造器帧，丢弃其及内侧帧）。

### 2.2 `Error.stackTraceLimit` 未生效 + `own_value` 不认原生构造器

`stackTraceLimit` 已挂属性（10），但 `build_error_stack` 未按它截断。
实现 `Vm::error_stack_trace_limit()` 读取该值后暴露出**更底层缺陷**：

```text
Vm::own_value(idx, key)  // 只处理 Closure 与 Ordinary
```

`NativeCtor`/`NativeFn` 的自有属性存在各自的 `properties` 表，但 `own_value`
**完全不认**这两个变体 ⇒ `own_value(Error, "stackTraceLimit")` 恒 `None`
（连同 `Array.from`、`Promise.withResolvers` 等静态面读取都受影响）。
**修复**：`own_value` 增加 `NativeCtor`/`NativeFn` 分支。

### 2.3 【连带】`super(args)` 到内建父类丢实参

```text
class A extends Error { constructor(m) { super(m); } }
new A('aa').message     // node "aa"；aluka ""（修复前）
```

`do_construct_this` 的内建父类分支把父类构造写入的自有属性复制到 `this` 时用
`own_entries`（**只含可枚举**），而轮九十八把 Error 的 `message`/`stack` 改为
**不可枚举** ⇒ 复制被整体过滤。**修复**：改用 `own_entries_all`，
复制后 `refresh_error_stack_name` + `refresh_error_enumerability`。

### 2.4 【连带】原生构造器自有面不可见

```text
Object.getOwnPropertyNames(Error)
  node  ["length","name","prototype","captureStackTrace","prepareStackTrace","stackTraceLimit"]
  aluka []                                     ← 完全不可见
Object.keys(Error)     node ["stackTraceLimit"]   aluka []
for (k in Error)       node stackTraceLimit       aluka prototype,captureStackTrace,…
```

三处独立路径（`interpreter.rs` 的 `Object.keys` 专用分支、`Object.getOwnPropertyNames`
分支、`property.rs::enumerate_for_in_keys`）都缺 `NativeCtor`/`NativeFn` 处理；
且 `NativeCtor`/`NativeFn` 结构**没有 `non_enum` 字段**（无法表达"`prototype` 不可枚举
而 `stackTraceLimit` 可枚举"）。**修复**：为两个变体补 `non_enum` 字段
（`alloc_native_ctor` 默认把 `prototype` 列入），三条枚举路径统一按 `non_enum` 过滤，
键序对齐 Node（`length`,`name`,`prototype` 前置，其余**字典序**保证 HashMap 迭代确定性）；
另补 `Error.prepareStackTrace`（默认 `undefined`、不可枚举）。

---

## 3. 达成证据（Node 22 逐项对拍）

### 3.1 `Error` 构造器静态面（8 项逐行一致）

```text
                                              node                              aluka（修复后）
names  getOwnPropertyNames(Error)             ["length","name","prototype",      同左        ✓
                                               "captureStackTrace",
                                               "prepareStackTrace","stackTraceLimit"]
keys   Object.keys(Error)                     ["stackTraceLimit"]                同左        ✓
desc   captureStackTrace                      {w:true,e:false,c:true}            同左        ✓
desc   stackTraceLimit                        {value:10,w:true,e:true,c:true}    同左        ✓
       typeof Error.captureStackTrace         function                           同左        ✓
       'captureStackTrace' in Error           true                               同左        ✓
forIn  for (k in Error)                       ["stackTraceLimit"]                同左        ✓
```

### 3.2 `stackTraceLimit` 截断与 `captureStackTrace`

```text
                                              node        aluka（修复后）
Error.stackTraceLimit = 3; deep(5).stack 行数  4           4     ✓
Error.stackTraceLimit = 0; 行数                 1（仅首行）  1     ✓
captureStackTrace 后 typeof e.stack            "string"    "string"  ✓
captureStackTrace 后 Array.isArray(e.stack)    false       false     ✓
captureStackTrace(this, MyErr) 含 MyErr 帧      否          否        ✓
captureStackTrace(this, MyErr) 含 caller 帧     是          是        ✓
captureStackTrace 首行                          "Error: c"  同左     ✓
```

### 3.3 连带修复：`super(args)` 到内建父类

```text
class A extends Error { constructor(m) { super(m); } }   new A('aa').message
  node "aa"   aluka（修复前）""   aluka（修复后）"aa"  ✓
（同项覆盖 B 带 name 覆写、C 无自定义构造器三形态，均一致）
```

### 3.4 【重要】Express 端到端回归与 `prepareStackTrace` 机制

`express_e2e_test::express_six_scenes_match_node22_oracle` 在本轮中途**失败**：

```text
TypeError: r.getFileName is not a function
    at <module> (demo/express-demo/aluka_build/.e2e_app_*.bc)
```

**根因**（真实包用法，`node_modules/depd/index.js::getStack`）：

```js
Error.prepareStackTrace = prepareObjectStackTrace;   // 钩子：callsite → 结构化数组
Error.stackTraceLimit = Math.max(10, limit);
Error.captureStackTrace(obj);
var stack = obj.stack.slice(1);                      // ← 期望 **数组**
Error.prepareStackTrace = prep;
```

Node 的 `stack` **并非恒为字符串**：`Error.prepareStackTrace` 为函数时，`stack` 是其
**返回值**（`depd` 借此拿 callsite 数组并调用 `getFileName()` 等）。上一轮把 stack 固定为
字符串、本轮把 `captureStackTrace` 也改成字符串，都**忽略了该钩子** ⇒ `depd` 拿到字符串后
`.slice()` 得到字符、`getFileName` 不可调用。

**修复**：`fill_error_stack` 按 Node 语义分流——钩子为函数时构造 callsite 数组
（新增 `build_callsite_array`，逐帧带 `_file`/`_funcName`）并以 `(err, callSites)` 调用钩子、
把**返回值**写为 `stack`；钩子缺失才生成字符串。`callsite_method` 补齐真实包所需方法面
（`getFileName`/`getLineNumber`/`getColumnNumber`/`getFunctionName`/`getTypeName`/
`getThis`/`getEvalOrigin`/`isEval`/`isNative`/`isConstructor`/`isToplevel`/`toString`）。

**复验**：`express_e2e_test` → `1 passed; 0 failed`（6 场景与 oracle 逐行一致）。

---

### 3.5 【重要】两处门禁捕获的连带回归（均已修复）

**回归 A：Express 端到端**（详见 §3.4）——`Error.prepareStackTrace` 未实现，
`depd` 拿不到 callsite 数组。已修复（钩子分流 + callsite 方法面）。

**回归 B：test262 `m72-built-ins-Boolean-prototype-S15.6.3.1_A4`**：

```text
FAIL m72-built-ins-Boolean-prototype-S15.6.3.1_A4.js
  Test262Error: The value of !Boolean.propertyIsEnumerable('prototype') is expected to be true
```

**根因链**（三处，均为"原生构造器/函数原型面"缺失）：

1. `obj_prop_is_enum` 只检查 `Ordinary` 的 `non_enum`，未认 `NativeCtor`/`NativeFn`
   ⇒ `Boolean.propertyIsEnumerable('prototype')` 误为 true。改用统一的
   `key_is_non_enumerable`（已覆盖全部变体）。
2. `proto_getter!` 宏以 `alloc_ordinary_with_proto(None)` 创建内建原型
   ⇒ `Function.prototype` 的 [[Prototype]] 为 `null`，`Object.prototype` 方法不可达。
   改为 `vm.object_prototype`。
3. 本引擎函数方法面是 **CALL_METHOD 硬编码链 + fn_proto 属性表**，不自动沿原型链查找
   ⇒ `(function(){}).valueOf` / `.propertyIsEnumerable` / `.isPrototypeOf` 一概缺失。
   在 `fn_proto` 上显式挂继承键并转发到 `Object.prototype` 同名 handler；
   `call_method_dispatch` 增加函数 receiver 的 `Object.prototype` 方法兜底。

**复验**：

```text
$ ALUKA_T262_FILTER=S15.6.3.1_A4 test262_subset_test
PASS m72-built-ins-Boolean-prototype-S15.6.3.1_A4.js
test262 subset: 1/1 passed（0 invalid）

（探针对拍，逐行一致）
fnPie=false / fnHasOwn=true / fnIsProtoOf=false / fnToString=function / fnValOf=function / arrPie=false
t1=function t2=function t3=function t4=valueOf call=function
```

---

## 4. 门禁（全绿）

```text
$ cargo fmt --all --check                                    FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings    CLIPPY_EXIT=0
$ cargo test --workspace --all-features --no-fail-fast -- \
    --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
（见 §4.1）
```

### 4.1 全量结果（实测）

```text
TEST_EXIT=0
suites=92   passed=653   failed=0
test node22_conformance_matches_node_stdout ... ok
test express_six_scenes_match_node22_oracle ... ok    ← 本轮回归 A 已修复
test test262_subset_conformance ... ok                ← 本轮回归 B 已修复
```

---

## 5. `git diff` 复审

```text
 crates/aluka-vm/src/heap.rs        | Error 静态面/stack 重构：alloc_error_instance 拆分、
                                       build_error_stack_with（constructorOpt + limit）、
                                       build_callsite_array、prepare_error_stack、fill_error_stack
 crates/aluka-vm/src/call.rs        | do_construct_this 内建父类分支改 own_entries_all + 刷新
 crates/aluka-vm/src/ops.rs         | typed_error / attach_error_proto：name 归原型 + stack 同步 + 收口
 crates/aluka-vm/src/property.rs    | own_value 认 NativeCtor/NativeFn；枚举面 include_non_enum 变体；
                                       原生构造器枚举与键序；for-in 过滤
 crates/aluka-vm/src/interpreter.rs | Error.prototype name/message；Object.keys / getOwnPropertyNames
                                       的 NativeCtor/NativeFn 分支
 crates/aluka-vm/src/builtins/global/{mod,object,error}.rs | captureStackTrace 委托 fill_error_stack；
                                       prepareStackTrace/stackTraceLimit 注册；callsite 方法面
 crates/aluka-vm/src/worker_clone.rs | serialize_error 读有效属性
```

逐块审核：改动集中在 Error 子系统与其依赖的枚举/自有面基础设施；无夹带改动、无调试残留。

---

## 6. 仍未修复（登记）

| # | 项 | 说明 |
|---|---|---|
| 1 | `stack` 帧**行号/列号** | 置 0/1（无调试信息与源映射）；Node 为真实行列 |
| 2 | `getFunctionName` 等精度 | 依赖调用链函数名（合成名如 `A_constructor` 可能外泄）；`getThis`/`getTypeName` 未跟踪接收者 |
| 3 | `prepareStackTrace` 的 `callSites` 为简化对象 | 非 V8 `CallSite` 实例（原型链不同），方法面覆盖真实包所需子集 |
| 4 | 无 exe 槽位 / 根目录 `aluka.exe` 陈旧 / `--capabilities` 报 `native: 0` / `deepEqual` 宽松语义 / 事件循环真实时钟 | 承接轮九十五–九十七登记 |

