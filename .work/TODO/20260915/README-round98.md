# 2026-09-15 · 续轮 TODO（M7.2 轮九十八：Error `stack` 与属性面 Node 对齐 + `getOwnPropertyNames` 修复）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README.md](./README-round97.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载）　|　**权威 Oracle**：Node.js 22 LTS（本机 v22.3.0）

**本轮范围**：轮九十五/九十六/九十七 §6 登记的「`Error` 实例缺自有 `stack`」。
调查中连带发现并修复 **`Object.getOwnPropertyNames` 整体过滤不可枚举属性** 的既有缺陷
（影响面大于原始登记项）。

---

## 1. 待办与结果

| # | 待办 | 结果 |
|---|---|:---:|
| 1 | `Error` 实例带自有 `stack`（字符串） | `[x]` §2.1 |
| 2 | `stack` 首行 = `Name: message`（子类为 `TypeError: …`） | `[x]` §2.1 |
| 3 | `Object.keys(err)` 为空集 / `name`·`message`·`stack` 非枚举 | `[x]` §2.2 |
| 4 | `new Error()` 与 `new Error('')` 属性面区分 | `[x]` §2.3 |
| 5 | `Object.getOwnPropertyNames` 含不可枚举自有属性（连带缺陷） | `[x]` §2.4 |
| 6 | 函数对象自有面键序 `["length","name","prototype"]` | `[x]` §2.4 |
| 7 | `Error.prototype` 补 `toString` | `[x]` §2.5 |
| 8 | 门禁（fmt / clippy / 全量 test） | `[x]` §4 |

---

## 2. 缺陷根因与修复

### 2.1 `Error` 无自有 `stack`（原登记项）

**Node 语义**（本机实测锁定）：

```text
new Error('boom')                    → Object.getOwnPropertyNames = ["stack","message"]
                                       typeof e.stack = "string"，首行 "Error: boom"
new TypeError('t-msg')               → 首行 "TypeError: t-msg"，name = "TypeError"
new Error() / new Error(undefined)   → 属性面仅 ["stack"]，首行 "Error"（无 ": " 段）
new Error('')                        → ["stack","message"]，首行 "Error"
e.stack = 'x'                        → 可写（普通数据属性）
```

此前 aluka 的 `alloc_error_instance` 只落 `message`+`name`，**没有 `stack`**
（`typeof e.stack === "undefined"`）。

**修复**（`crates/aluka-vm/src/{heap,call,microtask,ops,interpreter}.rs`）：

- `alloc_error_instance_with(message, has_message)`：按 Node 键序先落 `stack`
  （**创建时刻**捕获）再落 `message`；
- 新增 `alloc_error_instance_no_message()`：`new Error()` / `new Error(undefined)`
  走此路径（不落自有 message，`stack` 首行无 `: ` 段）；
- `build_error_stack`：首行 `Name(: message)` + 调用帧——帧取自
  `call::call_chain_snapshot()`（`invoke_function` 已维护的调用链），
  格式对齐 V8（`    at fn (file:0:1)`），无函数帧时 `    at <module> (file)`；
  **行号/列号/源映射为尽力而为**（无调试信息，行号置 0、列号置 1）；
- `refresh_error_stack_name`：子类改名（`attach_error_proto`）后同步首行；
- `CALL_CHAIN` 帧名改 `Rc<str>`：调用链在**每次函数调用**维护，`String` 会在
  热路径产生大量克隆分配。

### 2.2 可枚举性：`Object.keys(err)` 应为空集

Node 的 `name`/`message`/`stack` 均**不可枚举**（`Object.keys(err) === []`），
此前 aluka 三者都可枚举（`Object.keys(err)` 得 `["message","name"]`）。
新增 `refresh_error_enumerability`：在**构造收尾**（`new Error/子类`、`typed_error`、
`attach_error_proto` 返回前）统一标记三键不可枚举。

### 2.3 `name` 归属：原型而非实例

Node 的实例**没有**自有 `name`（读值沿链命中 `Error.prototype.name`）。此前 aluka
把 `name` 落实例自有，致 `getOwnPropertyNames` 多出一项。修复：`Error.prototype`
补 `name="Error"`（非枚举）与 `message=""`；实例不再落 `name`。

### 2.4 【连带既有缺陷】`Object.getOwnPropertyNames` 过滤了不可枚举属性

**实测**（修复前）：

```text
node  : const o={}; Object.defineProperty(o,'hidden',{value:1,enumerable:false}); o.visible=2;
        Object.getOwnPropertyNames(o) → ["hidden","visible"]
aluka :                                        → ["visible"]      ← 缺不可枚举键
node  : Object.getOwnPropertyNames(function(){})  → ["length","name","prototype"]
aluka :                                            → ["prototype"]
```

**根因**：`own_entries` / `own_properties` 把「不可枚举」当作「不存在」统一过滤，
而 `Object.getOwnPropertyNames` 复用了同一函数（`keys` 与 `getOwnPropertyNames`
在实现里只差一个字符串包装 `length` 特判）。

**修复**：为两者各加 `include_non_enum` 变体（`own_entries_all` /
`own_property_names`，内部共用 `keep` 谓词），`Object.getOwnPropertyNames`
改走新变体；`Object.keys`/`JSON.stringify`/`for-in` 路径**行为不变**。
另按 Node 键序把函数对象自有面补为 `["length","name","prototype"]`
（`length`/`name` 在实现中由 `get_property` 合成、不入 `properties`，故在枚举面按序前置）。

### 2.5 `Error.prototype.toString`

Node 的 `Error.prototype` 自有键为 `["constructor","name","message","toString"]`，
aluka 缺 `toString`。按规范 S20.5.3.4 实现（`name`/`message` 按有效值组合，
两者皆空 → 空串），供 `String(err)` / 模板串插值 / 未捕获渲染复用。

---

## 3. 达成证据（Node 22 逐项对拍）

### 3.1 Error 属性面与 stack（探针逐行一致）

```text
                                              node                         aluka（修复后）
A  getOwnPropertyNames({a:1})                 ["a"]                        ["a"]               ✓
C  getOwnPropertyNames(o)（含不可枚举）        ["hidden","visible"]         ["hidden","visible"] ✓
C- Object.keys(o)                             ["visible"]                  ["visible"]         ✓
D  getOwnPropertyNames(function(){})          ["length","name","prototype"] 同左               ✓
E  getOwnPropertyNames(new Error('m'))        ["stack","message"]          同左                ✓
F  getOwnPropertyNames(new Error())           ["stack"]                    ["stack"]           ✓
G  stack 描述符 enumerable                     false                        false               ✓
H  getOwnPropertyNames(new TypeError('t'))    ["stack","message"]          同左                ✓
I  new TypeError('t').name / stack 首行        TypeError|TypeError: t       同左                ✓
J  getOwnPropertyNames(Error.prototype)       ["constructor","name","message","toString"]  同左 ✓
K  stack/message 描述符 enumerable             false|false                  false|false         ✓
L  String(new Error('m'))                     "Error: m"                   "Error: m"          ✓
M  模板串 `x ${new TypeError('t')}`           "x TypeError: t"             同左                ✓
P  String(new Error(''))                      "Error"                      "Error"             ✓
Q  try{null.x}catch(e){e.name, e.stack 首行}   TypeError|TypeError: …       同左                ✓
```

关键语义（Node 实测锁定，均一致）：`typeof err.stack === "string"`、首行
`Error: boom`（子类 `TypeError: …`）、`e.stack` 可写、`new Error()` **无**自有
message（首行无 `: ` 段）、`Object.keys(err) === []`、`getOwnPropertyNames`
含不可枚举自有属性。

### 3.2 结构化克隆（连带回归修复）

```text
structuredClone(new TypeError('bad')) → instanceof Error / TypeError 均 true，name = TypeError
```

修复前 aluka 得 `instType=false`、`name=Error`：`serialize_error` 读**自有** name，
而本轮把 name 改为归属原型。已改为读**有效属性**（沿原型链）。

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
test test262_subset_conformance ... ok
```

修复过程中**一次真实回归**（已修复并复验）：`m5_semantics_test::clone_getter_invalid_date_error_matches_node`
因 `name` 改归原型而失败（`structuredClone(new TypeError('bad')) instanceof TypeError`
由 true 变 false）——根因是 `worker_clone::serialize_error` 读**自有** `name`
（`own_text`），name 迁移后恒得 `"Error"`。已改读**有效属性**（沿原型链），
并移除随之失效的 `own_text`。复验：`m5_semantics_test` 8/8 通过。

**demo/taskboard-demo 端到端**：`aluka run e2e.js` 与 Node 仍 54 行逐字节 IDENTICAL。

---

## 5. `git diff` 复审

```text
 crates/aluka-vm/src/heap.rs                | alloc_error_instance 拆分（含/不含 message）+ stack 生成 + 刷新辅助
 crates/aluka-vm/src/call.rs                | 构造分派传 has_message；CALL_CHAIN 改 Rc<str>；call_chain_snapshot
 crates/aluka-vm/src/ops.rs                 | typed_error / attach_error_proto：name 归原型 + stack 同步 + 收口
 crates/aluka-vm/src/interpreter.rs         | Error.prototype 补 name/message（非枚举）
 crates/aluka-vm/src/property.rs            | own_entries/own_properties 加 include_non_enum 变体 + 函数对象键序
 crates/aluka-vm/src/builtins/global/{mod,object,error}.rs | getOwnPropertyNames 走新变体；Error.prototype.toString
 crates/aluka-vm/src/worker_clone.rs        | serialize_error 改读有效属性（name 归原型后的连带修正）
```

逐块审核：仅含本轮 Error 属性面修复 + 连带的 `getOwnPropertyNames` 修复及对本次改动的
适配（`serialize_error`、`CALL_CHAIN` 的 `Rc<str>` 为必要调整，均已在注释说明）；无夹带
改动、无调试残留。

---

## 6. 仍未修复（登记）

| # | 项 | 说明 |
|---|---|---|
| 1 | `stack` 帧行号/列号 | 置 0/1（无调试信息与源映射），帧仅含函数名 + 入口文件；Node 为真实行列 |
| 2 | `Error.captureStackTrace` 覆写形态 | 仍写「调用点数组」（非字符串），与 `stack` 字符串形态不一致（既有实现） |
| 3 | `Error.stackTraceLimit` 未生效 | 已挂属性（10），但 `stack` 生成未按其截断 |
| 4 | 无 exe 槽位 / 根目录 `aluka.exe` 陈旧 / `--capabilities` 报 `native: 0` / `deepEqual` 宽松语义简化 / 事件循环真实时钟 | 承接轮九十五–九十七登记 |

