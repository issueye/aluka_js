# 2026-09-10 · M5 独立复审（评审轮）

> 总 TODO 见 [../README.md](../README.md)；上一轮 M5 评审见
> [../20260909/README.md §15](../20260909/README.md)（HEAD `703a23e`）；
> 其后的修复轮 round5/6/7 见 `../20260909/README-round{5,6,7}.md`。

**当前里程碑**：M5 多线程并发与系统级扩展（复审判定）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 0. 评审基线与环境（重要限制）

| 项 | 值 |
|---|---|
| 评审对象 | HEAD `8db31df`（master），工作树干净 |
| 被审提交 | `3171923`(m5.3) → `dfcce3f`(m5.2) → `281647e`(m5.1)，即上一轮建议的修复顺序 |
| **本机 Node** | **v22.3.0**（`nvmd` 可用版本最高 22.3.0，**无 22.23.1**） |
| 关键推论 | `node:sqlite` 自 **v22.5.0** 才引入 → **本机不存在 sqlite oracle**；实测 `require('node:sqlite')` → `ERR_UNKNOWN_BUILTIN_MODULE` |
| 方法 | 四路源码勘察（M5.1/M5.2/M5.3 专项 + 证据链审计）＋ 本机实测（M5 差分门禁、全量门禁三连、跨引擎对照探针） |

> **口径声明**：本报告中所有「与 Node 对照」的实测结论，对照对象均为
> **Node v22.3.0**。凡是依赖 Node ≥22.5（sqlite）或 ≥22.12（`require(esm)`）
> 的结论，本机**无法验证**，只能判定"仓库内不可复现"，不能判定真伪。

---

## 1. 结论总表

| 项 | README 现登记 | 本轮复核判定 | 一句话依据 |
|---|---|---|---|
| **M5.1** worker_threads | `[~]` 结构化克隆闭环 | **部分证实，`[~]` 应维持** | 结构化克隆骨架为真且 22 行逐字对拍**本地复现成功**；但 5 类 Node 语义偏离 + 1 项新发现缺陷，验收用例覆盖度不及声称 |
| **M5.2** cluster + 端口共享 | `[~]` P0 已关闭 | **`[~]` 维持，「P0 已关闭」口径需收窄** | Content-Length 响应的 10s 症状确已关闭；**同类缺陷在 bodyless 响应上原样残留**（204 实测 10005ms）且新增 `status=0` 伪成功 |
| **M5.3** node:sqlite | **`[x]` 已闭环** | **证据面不成立，建议回退为 `[~]`** | 唯一"真对拍"用例在本机（及任何无 Node ≥22.5 的环境）**静默跳过比对**，实测 0.60s 通过；全仓无 44 行 oracle 入库、无 sqlite 语料 |
| **M5.4** node:test 进阶 | `[ ]` 未闭环 | **证实，维持 `[ ]`** | Timer Mock / 报告接线 / LCOV / CLI 入口均未落地；登记诚实 |

**总判定：M5 仍不能整体验收。** 与上一轮相比有实质进步（结构化克隆、端口共享、
真多进程、sqlite 语义面大幅 Node 化），但三项 `[~]`/`[x]` 的**验收证据强度**
均低于 README 的表述。

---

## 2. 门禁与对拍实测（自跑，非引用）

### 2.1 M5 差分门禁（权威通道）

```bash
$env:ALUKA_CONF_FILTER="m5"; cargo test -p aluka-cli --all-features --test conformance_node22_test -- --nocapture
```

```
PASS  20-m5-worker-threads.cjs
PASS  21-m5-cluster-http.cjs
PASS  25-m5-structured-clone.cjs
Result: 3/3 passed, 0 invalid          # 2.36s
```

✅ **M5 三用例差分对拍确实全绿**，且 `21-m5` 在 bc 模式下 2.36s 内完成
（上一轮记录为 12.1s）——fetch 提速在集群用例上真实可见。

### 2.2 `25-m5-structured-clone.cjs` 逐字节比对（手工）

```bash
node 25-m5-structured-clone.cjs  > n25.out    # exit=0, 22 行
aluka 25-m5-structured-clone.cjs > a25.out    # exit=0, 22 行
Compare-Object n25.out a25.out                # → IDENTICAL
```

✅ **「22 行逐字一致」本地独立复现成功**（Node 侧 5 次运行 MD5 一致，确定性 OK）。
**但输出内容暴露用例设计问题**（见 §3.4）：22 行里有 11 行是**事件级联重复**
产生的，其中 `nomenum:` 一行打印的是 **transfer 结果**而非该段声称验证的载荷。

### 2.3 全量门禁三连

| 命令 | exit | 结果 |
|---|---|---|
| `cargo fmt --all --check` | 0 | 无 diff |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | **0 warning / 0 error** |
| `cargo test --workspace --all-features` | 0 | **578 passed / 0 failed / 1 ignored**（397s，81 目标全 ok） |

- 与 README 声称的 `561 passed / 0 failed`：**0 failed 成立**；578 > 561 属**快照过期**
  （多出 17 个 `#[test]`/Doc-test），**不是**语料扩容导致（conformance 862 例聚合在
  单个 `#[test]` 内，只贡献 1 个 passed）。
- conformance 全量单跑：**`Result: 862/862 passed, 3 invalid`**。

### 2.4 conformance 语料的真实覆盖面（新发现）

- 语料目录共 **1035** 个 `.cjs/.mjs`，但 harness **只递归一层**
  （`conformance_node22_test.rs:123-143`、`:190-215`）：实枚举 = 27 顶层 + `gen/` 直接子文件 838 = **865 = 862 + 3** ✅
- 差额 **170 个文件全部位于 `cases/gen/deviations/`**（`gen/` 下唯一子目录）
  → **该目录结构上从不被门禁枚举**。
- 即：**「conformance 全量绿」= 自选语料的全绿**；170 例已知分歧（M7.2 登记为
  175 例分歧、差值为快照与文件数口径差）被隔离在门禁之外。该隔离本身已在
  `cases/gen/DEVIATIONS.md` 与 [README.md §待办 8](./README.md) 诚实登记，
  但**任何引用"conformance 全绿"作为 M5 验收证据的表述都应附带这一限制**。
- 3 条 `INV` 为 `03-require-esm.cjs`、`15-test-runner.cjs`、`16-m7-test-core.cjs`，
  成因均为**本机 Node 版本偏低**（node rc=1），非 aluka 侧失败。✅ 该通道
  **逐条打印 INV 并计入计数，不是静默跳过**（与 §4.2 的 `assert_e2e_matches_node`
  不同）。

---

## 3. 分项复核

### 3.1 M5.1 结构化克隆 —— 骨架为真，语义面有 5 处偏离

**证实（源码 + 实测）**：
- 真引用表（memo）而非仅防递归：`worker_clone.rs:142-156` `Ser.objects`，
  `:204-210` 命中写 `T_REF`，反序列化 `:551-557` 按索引回填 → 循环与共享引用
  同时成立（用例 `cyc:true`、`share:true` 实测为真）。
- transfer + detach 真实：源 `byteLength` 归零、视图 `length` 归零、`DataView`
  属性读抛 TypeError（`property.rs:638-640` + `typed_array.rs:305-313`）。
- `markAsUntransferable` 真拦截（`worker_clone.rs:26-38`、`:105-112`）。
- 函数/Symbol/Promise → DataCloneError，三者实测 `name` 与 Node 一致。
- 克隆表**每消息新建**，跨线程只传 `String`（base64），无堆索引/裸指针跨线程，
  两文件 `unsafe` 命中 0；base64 承载不吞非 UTF-8 字节。
- 「纯消息 worker 保活」修复落地于 `worker_threads.rs:1161-1180`
  （`pp_waiting` 作继续循环条件）。

**偏离 Node（实测，跨引擎对照）**：

| 探针 | Node v22.3.0 | Aluka | 级别 |
|---|---|---|---|
| `postMessage({get a(){return 1}})` | 投递成功，worker 收到 `a=1`（**调用 getter**） | **抛 DataCloneError** | P1 |
| `postMessage({s:SharedArrayBuffer}, [s])` | **抛 DataCloneError** | 投递成功，**源 `byteLength` 变 0（共享缓存被清零）** | P1 |
| `postMessage({x:1}, [ab])`（ab 不在消息图内） | 源 `ab.byteLength=0`（**仍 detach**） | 源 `ab.byteLength=4`（**未 detach**） | P1 |
| `postMessage({d:new Date(NaN)})` | worker 侧 `Invalid Date` | 经 i64 饱和转换 → 1970（`:244-245`） | P1 |
| `Error` 实例 | `instanceof Error` true、保留 stack/cause | 退化为普通对象（heap 无 Error 变体） | P1 |

代码推定（未实测）：稀疏数组空位与数组自有属性丢失（`serialize_array`
不读 `properties`，`:283-297`）、Map/Set 键被字符串化。

**新增缺陷（本项目未登记，本次实测发现）**：
> **主线程存在待触发定时器时，worker→主线程消息在该定时器触发前不被投递。**

```js
// worker: parentPort.on("message", () => parentPort.postMessage("alive"));
const w = new Worker(__filename);
w.on("message", (m) => console.log("MAIN got: " + m));
w.postMessage({ a: 1 });
setTimeout(() => { console.log("tick-1000"); w.terminate(); }, 1000);
```

| | Node | Aluka |
|---|---|---|
| 定时器 1000ms | `MAIN got: alive` → `tick-1000` | **只有 `tick-1000`（消息永久丢失）** |
| 定时器 2000ms（不 terminate） | `MAIN got: alive` → `tick-2000` | `tick-2000` → **`MAIN got: alive`（被推迟到定时器之后）** |
| 无定时器 | 立即投递 | 立即投递（正常） |

即主线程事件循环在**有待触发定时器时会驻留等待定时器**，而非同时轮询 worker
通道。任何 `setTimeout/setInterval` + worker 混用的程序都可能丢消息或收到
严重延迟的消息。**该缺陷 `25-m5` 用例无法抓到——它完全由消息驱动、没有待触发
定时器。**

### 3.2 M5.2 fetch —— 「P0 关闭」口径需收窄

**证实**：round6 的根因链（10s 读超时叠加，而非死锁）正确，时序数字可独立复算
（phase9 串行 7 请求 ×10s ≈ 70s ≈ 71s；双 fetch 串行 20s ≈ 20024ms）。
`Connection: close` 服务端不关连接的 Node 语义仍是遗留，登记诚实。

**P0 残留（实测）**：

```js
// 服务端：s.statusCode = 204; s.end();
/len  : status=200 body="ok"  elapsed=3ms       ← 修复生效
/204  : status=204 body=""    elapsed=10013ms   ← 仍空等 10s
```

**对照实验（决定性）**：**同一个 aluka 服务端**，改由 Node 客户端请求：

```
/len  : status=200 body="ok" elapsed=46ms
/204  : status=204 body=""   elapsed=5ms        ← Node 5ms
```

→ 缺陷在 **Aluka 的 fetch 客户端**，不在服务端。`response_complete` 只实现了
RFC 9112 §6.3 四条定界规则中的两条（Content-Length / chunked），`204/304/1xx/HEAD`
落回读超时兜底；而**本仓 `http/wire.rs:189-216` 已有更完整的实现未被复用**。

**新增缺陷（实测）**：对**只 accept 不发字节**的服务端，fetch 在 10s 后返回一个
**伪造的成功响应**而非报错：

```
silent: status=0 ok=false statusText="" bodyLen=0 elapsed=10007ms
```

`status===0` 在 Node 中只属 opaque 响应，正常 fetch 不可能出现；Node 会
reject。根因是读循环把一切 IO 错误/EOF 都当"响应结束"
（`global/fetch.rs:717-733`）且无状态行时 `unwrap_or(0)`（`:734-747`）。

**代码推定（未实测，供 owner 复核）**：chunked 终止块用 5 字节裸扫描
（`fetch.rs:775`）—— body 数据区含字面 `0\r\n\r\n` 会提前截断，带扩展/trailer
的终止块则永不匹配；fetch 全同步阻塞 → 同进程自请求拿不到响应、流式响应可
无限阻塞 VM；http server `listen` 错误**同步**发射且载体为字符串
（`http/server.rs:247-250`），"listen 后再挂 error handler"的 Node 文档写法会崩溃；
Windows 上 cluster 靠 `SO_REUSEADDR` 共享端口会**静默丢失 EADDRINUSE**。

### 3.3 M5.3 node:sqlite —— 实现本体质量明显提升，但验收证据不成立

**实现面证实**：`columns()` 五键与表达式列 `type=null` 正确
（`sqlite.rs:640-709`，`column_metadata` 特性已启用）；错误文本 Node 化
（errmsg 原文 / `ERR_SQLITE_ERROR` / `errcode` 扩展码 / `ERR_INVALID_ARG_TYPE`）；
`exec` 直写事务；bigint/blob/布尔/缺参/越位/未知命名参数的**形态**对齐。

**P0：验收证据不可复现（本次评审最重要的发现）**

判定链：
1. `common/mod.rs:144-166` `node_run` —— node 进程**退出码非 0 即返回 `None`**
   （stderr 丢弃）；
2. `common/mod.rs:183-192` `assert_e2e_matches_node` —— `if let Some(node_out) = ...`
   **无 `else`、无 panic、无版本门禁** → 比对整段跳过，函数照常返回本地输出；
3. `builtins_phase7_io_test.rs:220` 调用该 helper，`:222-230` 只有 5 条**硬编码
   本地锚点**（且期望文本与实现同源，属自证）。

实测：

```bash
cargo test -p aluka-cli --all-features --test builtins_phase7_io_test sqlite
# sqlite_node22_diff_e2e_matches_node ... ok        ← 0.60s 内通过
```

本机 Node v22.3.0 **没有 `node:sqlite`**，Node 侧不可能跑通，比对**必然被跳过**——
该用例在当前环境只能证明"实现自洽"，**不能证明与 Node 一致**。

配套问题：
- 仓库内**不存在** 44 行 oracle 产物（round5 提到的 `node-out.txt` 未入库），
  也无任何 `node:sqlite` conformance 语料 → **全仓没有一条可复现的 sqlite
  对拍证据**；
- 更结构性：即便将来有 Node ≥22.5，**Node 侧任何报错都会把"比对"降级为"跳过"**，
  测试永远不会因 Node 侧失败而失败。

**实测确认的语义偏离**：

| 探针 | Node 语义 | Aluka 实测 |
|---|---|---|
| `db.exec("BEGIN; INSERT; COMMIT")` 后 `db.isTransaction` | `false` | **`true`**（`:253-269` 首 token 启发式） |
| `db.prepare("SELECT :name").get({name:'Bob'})` | 默认**拒绝**裸名（需 `setAllowBareNamedParameters(true)`） | **成功返回 `{v:'Bob'}`** |
| `SELECT ? AS v` 绑 `Uint8Array` 后 `v instanceof Uint8Array` | `true`，`[object Uint8Array]` | **`false`**，`[object Object]`，`constructor` 缺失 |
| `catch (e) { e instanceof Error }` / `e.stack` | `true` / 有 | **`false` / `undefined`**（`alloc_ordinary` 造对象） |

**内部自相矛盾（关键）**：探针 `probes/node22_sqlite_probe.js:23` **依赖裸名参数**
（`{name:'Bob'}` 绑 `:name`）。若 Node 22.23.1 真如项目自述"默认不允许裸名"，
则该探针在 Node 上**必然抛错** → 与"44 行逐字一致"**逻辑上不可同时成立**；
若该探针在 Node 上能跑通，则项目对 Node 默认行为的登记有误。二者必有一错，
需 owner 用可复现证据澄清。

代码推定（未实测）：句柄/状态表以**裸堆索引**为键（`sqlite.rs:114-132`）——
活语句不根化其数据库、`db.close()` 只清 `STMTS` 不清 `ITERS/TXNS`、表项永不释放；
`columns()` 在异常路径静默返回 `[]`（`:641-644`）；`iterate()` 实为全量物化。

### 3.4 验收用例 `25-m5` 的覆盖度（与声称对照）

声称"类型面全覆盖"，实测**真实观测到**：undefined/null/string/number/BigInt、
嵌套对象、循环+共享引用、Map（仅字符串键）、Set（仅字符串元素）、Uint8Array、
transfer 后长度与首元素、detach 后 `ab.byteLength=0`/`ta.byteLength=0`/DataView 抛错、
函数/Symbol/Promise 抛错、markAsUntransferable 抛错、已 detach 再 transfer 抛错。

**声称覆盖但观测不到**：

- **「不可枚举属性」段（第 6 段）完全空转**：worker 的 `out` 字段集固定
  （`cases:13-28`），**从不读取 `plain2`**；实测该段输出的 `nomenum:` 行内容是
  **transfer 结果**，`plain2` 在所有 22 行中都不出现。
- `Date` 只判 `typeof === 'object'`（`cases:24`）——克隆成 `{}` 也会通过；
  值正确性零验证（且引擎 `getTime` 缺失使该验证无法用常规手段进行）。
- `RegExp` 只用 `source/flags`，`lastIndex` 未测。
- 仅 Uint8Array，无 Float64Array/BigUint64Array/DataView offset。
- 无 `Error`、无 SharedArrayBuffer、无 getter 属性、无稀疏数组/空位、
  无 `-0/NaN/Infinity`（载荷只有 42）、无 Symbol 作 key、无多视图
  `ta.buffer === ab` 身份断言。
- Map/Set 恰好只用字符串键/元素，**正好绕开键字符串化缺口**。

> 结论：22 行逐字一致是**真事实**，但它验证的是"这份特定探针在两个引擎上产生
> 相同字节"，而非"结构化克隆的类型面与 Node 等价"。

---

## 4. 跨项：证据链有效性审计（本次评审的方法论发现）

### 4.1 真正的 Node 对拍只有 3 处，127 处是本地自测

```
assert_e2e_matches_node 调用点：3（builtins_iter_protocol / builtins_json_stringify / builtins_phase7_io）
assert_e2e_matches_go   调用点：127
```

`assert_e2e_matches_go`（`common/mod.rs:196-198`）**仅是 `rust_pipeline_run` 的
兼容别名，完全不比对 Node**。因此 builtins e2e 家族的绝大多数"对拍"声称
（含 4 个 sqlite e2e 的全部"Node 22.23.1 实测"注释）**在自动化层面无 Node 对照**。

### 4.2 静默跳过通道清单

| 通道 | 是否静默 | 位置 |
|---|---|---|
| `assert_e2e_matches_node` | **是（真静默）** | `common/mod.rs:183-192`，无 `else`/无 panic |
| conformance `INV` | 否（逐条打印 + 计数） | `conformance_node22_test.rs:236-244` ✅ 诚实 |
| `gen/deviations/` 170 例 | 否（登记于 DEVIATIONS.md），但**结构性不参与门禁** | harness 只递归一层 |
| oracle 探针失败即 `return` | **是** | `four_quadrants_oracle_test.rs:87-95` 等（本次未触发） |

### 4.3 引擎级公共前提缺口

实测（HEAD 二进制）：

```
new Date(0).getTime        → undefined      (Node: 0)
new Date(0).toISOString    → undefined      (Node: function)
new Uint8Array([1,2]) instanceof Uint8Array → false
new Uint8Array([1,2]).constructor          → 缺失
Object.prototype.toString.call(u8)         → [object Object]  (Node: [object Uint8Array])
new Set([3,'3']).size                      → 1   (Node: 2)
new Map().set(3,'n').set('3','s').get(3)   → 's' (Node: 'n'，size 2)
```

根因与本日 TODO §待办 8 自述的**未收敛系统性缺口「原型方法属性读面」**一致。
影响：
- M5.3「blob→Uint8Array 对齐 Node」在探针绕行 `instanceof` 的前提下才成立；
- M5.1「类型面全覆盖」同样绕行；
- **`Map`/`Set` 键语义被破坏**（数字键与字符串键同键）会使任何以此为键的
  业务逻辑产生静默错误结果——这是本轮最严重的引擎级发现之一。

### 4.4 构建配置

`crates/aluka-cli/Cargo.toml` `default = []`，而 `src/bin/aluvm.rs:54` 无条件
使用可选依赖 `aluka_runtime` → **裸 `cargo build`/`cargo test` 直接编译失败**
（`E0433`），并非仅 conformance 用例失败。本日 TODO §0 已记录为"调用口径问题"，
但建议登记为**构建卫生缺陷**（`default` 下至少应能编译），以免任何省略
`--all-features` 的门禁运行产出无效结论。

---

## 5. 与上一轮评审跟踪项的闭环对照

| 上一轮跟踪项 | 状态 |
|---|---|
| M5.3 对拍补齐 | ⚠️ **形式补齐、实质未闭环**——用例存在但静默跳过（§3.3） |
| M5.2 P0（bc 模式 cluster+fetch 挂死） | ✅ 根因判定正确并修复 Content-Length 路径；⚠️ bodyless 残留 + 新增 `status=0`（§3.2） |
| M5.1 结构化克隆 | ✅ 主体落地且本地复现对拍；⚠️ 5 处语义偏离 + 定时器/worker 投递缺陷（§3.1） |
| M5.1 `postMessageToThread` 真线程分支 / eval worker / port ref-unref-start | ❌ 仍缺 |
| M5.1 文件头注释过时项（threadId 恒 0 等） | 未复核 |
| M5.2 IPC 面（`worker.send`/`isConnected`/`isDead`/exit code） | ❌ 仍降级；**新增** `cluster.worker.send/process`、`process.send`(`process.send` 全仓缺失)、`NODE_UNIQUE_ID`、id 复用、`settings.exec/args` 被忽略、`online/listening/disconnect` 事件缺失 |
| M5.2 `Connection: close` 服务端语义 | ❌ 仍缺 |
| M5.3 ctor options / 真预编译 / wrapper `isTransaction` | ❌ 仍缺 |
| M5.4 Timer Mock + CLI 运行器 | ❌ 仍缺（登记诚实，维持 `[ ]`） |
| 注释/文档过度声称 | ⚠️ `docs/builtins-manifest.md:61`「已完整实现（12 用例逐字对拍）」与现状（5 用例、1 名义对拍）不符 |

**应予表扬的两点**：round6 对 round4 误判（"bc 专有死锁"）的**自我纠正**有代码
依据且诚实；M5.4 未被"凑绿"，如实保留 `[ ]`。

---

## 6. 建议的登记动作与下轮顺序

### 6.1 建议修改的 README 口径（供 owner 决策，本报告未擅自改动总 README）

1. **M5.3 由 `[x]` 回退为 `[~]`**，并把「真对拍闭环 / 44 行逐字一致」改为
   「开发期曾于具备 Node ≥22.5 的环境实测；本仓无入库 oracle、当前环境静默跳过，
   证据不可复现」；
2. **M5.2「P0 已关闭」**改为「Content-Length 响应的 10s 症状已关闭；bodyless
   响应（204/304/HEAD）仍为 10s，且无字节响应会返回 `status=0` 伪成功」；
3. 任何「conformance 全量绿」的表述附注「170 例已知分歧隔离于
   `cases/gen/deviations/`，不参与门禁」；
4. `docs/builtins-manifest.md:61` 的 sqlite 描述与现状对齐。

### 6.2 建议的修复顺序（按"证据价值 ÷ 成本"）

1. **修 `assert_e2e_matches_node` 的静默跳过**（小改动、全局收益）：区分
   "node 缺失 → skip" 与 "node 存在但探针失败 → panic"；把 sqlite 44 行期望
   输出**以快照入库**并全量 `assert_eq`。这一条落地后，M5.3 的判定才可复核。
2. **补齐 fetch 定界规则**：复用 `http/wire.rs` 已有实现，覆盖 bodyless/HEAD；
   读循环区分"IO 错误"与"响应结束"，禁止 `status=0` 伪成功。
3. **修 worker→主线程消息的定时器饥饿**（§3.1）：主线程事件循环在有定时器
   待触发时仍须轮询 worker 通道。
4. **引擎原型属性读面**（根因项）：`constructor`/`instanceof`/`toStringTag`/
   `Date.prototype.*` —— 这是 M5.1/M5.3 声称的公共前提，也是 170 例偏差的主因。
5. M5.1 剩余语义偏离：getter 求值、SAB 拒绝 transfer、transfer list 中未出现
   于图内的 buffer 仍需 detach、Invalid Date、Error 实例。
6. M5.1 Map/Set 键语义（数字/字符串同键）——**建议提级为独立 P0**，影响面
   超出 M5。
7. **建议为 M5 补入并发回归用例**：≥2 连接并发 fetch、定时器+worker 混用、
   bodyless 响应、裸名参数 —— 现有三用例结构上覆盖不到这些面。

---

## 7. 评审方法与环境复现

```bash
# 差分门禁（M5 三用例）
$env:ALUKA_CONF_FILTER="m5"; cargo test -p aluka-cli --all-features --test conformance_node22_test -- --nocapture

# 全量门禁三连
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features          # 578 passed / 0 failed / 1 ignored

# 证据缺陷复现（本机 Node 无 node:sqlite 时该用例仍绿）
cargo test -p aluka-cli --all-features --test builtins_phase7_io_test sqlite
```

探针脚本（本次评审产出，位于会话 scratch）：跨引擎对照用的 `Map/Set` 键语义、
`Date`/`TypedArray` 属性面、`25-m5` 字节比对、204/无字节 fetch、worker+定时器
投递、结构化克隆 4 项边界（getter/SAB/未入图 transfer/Invalid Date）。

> **遗留未复核项**：`http/server.rs` 的 `Connection: close` 服务端关闭语义、
> fetch 流式响应阻塞、`cluster.worker.send`/`process.send` 崩溃路径均为**源码
> 判定**，未逐条实测；`case 21` 仍为单次 fetch，无 ≥2 连接并发回归。

---

## 8. 后续轮进展（修复轮，2026-09-10）

本报告 §6.2 的修复顺序已被执行到第 3 项，证据见
[./README.md §待办 9](./README.md)。摘要：

| §6.2 建议项 | 状态 |
|---|---|
| 1. 修 `assert_e2e_matches_node` 静默跳过 | ✅ 已修（三态区分 + 可见 `[SKIP node-e2e]`；负向对照证明"会真失败"） |
| 2. 补齐 fetch 定界规则 | ✅ 已修（204：10013ms → **4ms**；无字节服务端由 `status=0` 伪成功改为抛 `TypeError`） |
| 3. 修 worker 消息的定时器饥饿 | ✅ 已修（red→green 同工件对照，输出与 Node 逐字节一致） |
| 7. 补并发回归用例 | ✅ 已入库 `26-m5-fetch-bodyless.cjs` / `27-m5-worker-timer.cjs` |
| 4. 引擎原型属性读面 | ⬜ 未动（范围外，另立专项） |
| 5. M5.1 其余语义偏离 | ⬜ 未动 |
| 6. Map/Set 键语义 | ⬜ 未动（等级建议维持独立 P0） |

修复轮另发现两个**新的**独立缺陷（`aluka-npm` 项目根解析越界、测试基建
`node -e` 载荷被 shell 破坏导致长期假绿），已在 §待办 9 §6 登记，本轮**未修**
并附安全理由。
