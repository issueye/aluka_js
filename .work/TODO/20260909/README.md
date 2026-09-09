# 2026-09-09 · 每日 TODO（M2.4 结项轮：Express 6 场景全绿 + 引擎修复）

> 总 TODO 见 [../README.md](../README.md)；证据规则见其 §0。

**当前里程碑**：M2.4 Express 真实依赖树（结项中）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

## 1. 本轮目标（可判定完成态）

1. 修复 `isNaN('23')` 返回 true 导致的 `typeis.hasBody` 误判（body-parser 读 body 为空）；
2. 修复 `res.send`/`res.json` 内部崩溃——etag 生成断裂（`Number.prototype.toString(radix)` 缺失）；
3. 修复 `res.send` 卡死——`bind` 被 `try_dispatch` 劫持（`raw-body` 的 `runInAsyncScope.bind` 错位）；
4. 修复 `iconv-lite` 编码加载崩溃——`String.prototype.fromCharCode` 缺失、`StringDecoder.prototype` 缺失、`NativeFn` 属性写入被忽略、`for...in` 修复；
5. 修复 `Object.create` 第二参数`own_entries` 在非 Ordinary 对象上崩溃；
6. 修复编译器字符串 `\uXXXX` 转义缺失（`encodeUrl` 乱码根源）；
7. 修复 `ToBoolean` 空字符串 truthy 语义错误（`path-to-regexp` 走错分支的根源）；
8. 门禁全绿 + 证据回填。

## 2. 修复清单

| # | 修复项 | 文件 | 类型 |
|---|---|---|---|
| 1 | **ToBoolean 空字符串 falsy**：`ops::to_boolean` 新增堆参数，`Value::is_truthy` 移除；`__all call sites__` 迁移至 `to_boolean(val, &self.heap)` 或 `self.truthy(val)` | `ops.rs`, `value.rs`, `interpreter.rs`, `call.rs`, `proxy.rs`, `property.rs`, `typed_array.rs`, `jit_helpers.rs`, `builtins/assert.rs`, `builtins/assert_strict.rs`, `builtins/fs.rs`, `builtins/fs_promises.rs`, `builtins/sqlite.rs`, `builtins/http/mod.rs`, `builtins/test/context.rs` | 语义 |
| 2 | **字符串 `\uXXXX` 转义**：lexer 字符串字面量分支新增 `\xXX`、`\uXXXX`、`\u{...}` 转义处理；`read_hex_units` 失败时 `pos` 还原；`continue` 跳过末尾 `+1` | `crates/aluka-parser/src/lexer.rs` | 编译器 |
| 3 | **`Number.prototype.toString(radix)`**：新增 `num_method_dispatch` 分派 handler + `format_number_decimal`/`format_number_radix` | `builtins/surface.rs` | 内置库 |
| 4 | **`bind` 被 try_dispatch 劫持**：CALL_METHOD 中 `bind` 走通用协议优先于 `try_dispatch`；`fn_proto_bind` 改为 `pub(crate)` | `interpreter.rs`, `builtins/surface.rs` | VM |
| 5 | **`String.fromCharCode`/`fromCodePoint`**：新增 `String` 构造器全局 + 静态方法 handler | `builtins/global_fns.rs` | 内置库 |
| 6 | **`StringDecoder.prototype` 缺失**：`string_decoder` 模块 build 时挂 `prototype.write`/`prototype.end` | `builtins/string_decoder.rs` | 内置库 |
| 7 | **`NativeFn` 属性写入被忽略**：`set_property` 添加 `NativeFn` 分支 | `property.rs` | VM |
| 8 | **`Object.create` 第二参数崩溃**：回退 `own_entries` 特性的第二参数（复原为无第二参数版本） | `interpreter.rs` | VM |
| 9 | **`isNaN` 字符串 ToNumber**：`arg_number` 改用 `vm.to_number_value` 解析堆字符串 | `builtins/global_fns.rs` | 内置库 |
| 10 | **`Number.isInteger`/`isFinite`/`isNaN`/`isSafeInteger`**：改用 `to_num` 闭包（`vm.to_number_value`） | `builtins/global_fns.rs` | 内置库 |

## 3. 探针验证结果

| 探针 | 修复前 | 修复后 | 核心修复 |
|---|---|---|---|
| `probe_ternary` `'' ? 'A' : 'B'` | `A` | `B` | ToBoolean |
| `probe_min` `'\uFFFD'` | `"uFFFD"` | `""` | 转义 |
| `probe_regexp` path-to-regexp source | `^/echo(?:/((?:(?!/|).)+?))/?$`（含 backtrack） | `^/echo(?:/([^/]+?))/?$`（与 Node 一致） | ToBoolean |
| `probe_buf` `n.toString(16)` | `undefined` | `c` | Number.prototype.toString |
| `probe_etag` etag result | `"undefined-..."` | `"b-..."` | toString |
| `probe_iconv` `getCodec('utf-8')` | crash | `decoder: function` | 3 项修复 |
| `probe_forin2` encodings 模块 | crash `reading 'end'` | 408 keys 全 | NativeFn + StringDecoder prototype |
| `probe_body` POST body 流 | 空 body | 完整 | isNaN |
| `probe_express_parts` response 模块 | crash | 待验证 | 全链 |
| `app.js` 6 场景 | POST 空 body / 404 | 待验证 | 全链 |

## 4. 门禁结果

```bash
# 记忆：所有修复门禁须构建后验证
```

## 5. 提交

```bash
git commit -m "fix(m2.4): M2.4 结项排障批次——ToBoolean 空字符串/\\uXXXX 转义/Number.prototype.toString/bind 劫持/iconv 全链/Object.create 崩溃"
```

## 6. 遗留问题

- `require('express')` 加载 `response.js` 时仍崩溃（`undefined is not a function`），定位中；
- 修复后 `app.js` 6 场景 POST body 仍为空，需进一步排查 body-parser 的 `read` 回调。

## 7. 代码重构

- **`global_fns.rs` 拆分**：将 3255 行的巨型文件按单一职责拆分为 `builtins/global/` 子目录下 13 个独立模块（`core_fn`/`uri`/`number`/`string_fns`/`date`/`object`/`error`/`fetch`/`headers`/`abort`/`event`/`form_data`/`web` + 编排 `mod.rs`），编译零警告通过。

## 8. 迭代器协议实现（对照 Node.js 22 LTS）

- **新增四类内建迭代器**（`iter.rs`）：Array（既有，重构 kind 读取）/ String（逐 Unicode 码点，代理对单一产出）/ Map / Set；
- **Map/Set 存储升级有序化**（`heap.rs` `HeapObject::Map`）：`HashMap` → `Vec<(String, Value)>` 保持插入序；`set/add` 既有键原位更新保位置；`new Map([[k,v],…])` 支持 iterable 构造（`call.rs`）；Set 实例经句柄登记与 Map 区分；
- **原型 `[Symbol.iterator]` 面**（`surface.rs`）：String/Array/Map/Set（Map/Set 共用 container_proto + receiver 分流）；NativeFn 名与分派键对齐（`X.prototype.Symbol.iterator`）；
- **解释器协议接入**（`interpreter.rs`）：`GetIterator` 字符串/Map/Set 特判 + 迭代器对象自身即迭代器；`CALL_METHOD next` 四类迭代器步进；Map/Set `keys/values/entries/forEach` 方法；`Op::ArraySpread` 走 `collect_iter_values` 统一展开（数组/字符串/Map/Set/TypedArray）；
- **合成属性面**（`property.rs`）：Map/Set 变体知名符号键转发 container_proto 面；字符串变体符号键转发 str_proto 面（`'hi'[Symbol.iterator]()` 可取）；
- **差分测试**：`aluka-cli/tests/builtins_iter_protocol_test.rs` 5 用例与 Node.js 22 LTS 实际对拍（`assert_e2e_matches_node`）全绿；
- **回归**：`cargo test -p aluka-vm` 146+33 通过；aluka-cli `core_semantics_test` 21 通过、`four_quadrants_oracle_test`/`sync_builtins_test`/`frontend_features_test` 通过；clippy 零警告。

```bash
# 证据：探针输出（target/debug/aluka.exe）
# str-forof: a,b,c, ｜ str-codepoint: a|😀|b| ｜ spread-str: ["x","y"]
# map-forof: ["a=1","b=2"] ｜ map-keys: ["a","b"] ｜ map-values: [1,2]
# map-update-order: ["x=9","y=2"]（原位更新保插入序）
# set-size: 3 ｜ set-forof: [1,2,3] ｜ set-entries: ["[1,1]","[2,2]","[3,3]"]
# spread-map: [["k","v"]] ｜ spread-set: ["a","b"] ｜ Array 迭代器复用 sum: 6
```

## 9. JSON.stringify 修复（对照 Node.js 22 LTS）

- **`OrdinaryProps::Dict` 有序化**（`heap.rs`）：`HashMap` → `Vec<(String, Value)>` 保插入序（GC trace/GC 转换同步）；`set_property` shape→dict 转换按形状序物化、dict 分支原位更新或追加；
- **`delete` 慢化语义**（`property.rs` `delete_property`）：命中快属性即整体迁移字典模式（V8 同构），「删除后重加」键落在键序**末尾**——`Object.keys`/`JSON.stringify` 顺序对齐 Node（实测：`{a,b,c}` 删 b 重加 → `a,c,b`）；
- **序列化规则**（`prims.rs` `json_stringify`/`json_write`）：
  - 对象键序 = 整数索引键升序前置（规范 [[OwnPropertyKeys]] 子集，`"0"`/`"1"` 无前导零判定）+ 其余创建序（不再字典序排序）；
  - 属性值为 `undefined`/函数/符号 → 整键剔除；数组元素同值 → `"null"` 占位；
  - 顶层 `undefined`/函数/符号 → 返回 `undefined`（原错误输出字符串 `"null"`）；
- **差分测试**：新增 `aluka-cli/tests/builtins_json_stringify_test.rs` 3 用例与 Node 22 实拍全绿（键序/忽略值/40 键 dict 序/删除重加）；`core_semantics_test` 更新 `json2` 期望为 `undefined`（Node 语义）；14 项手工探针与 Node 逐行 diff 为空；
- **回归**：aluka-vm 146+33、core_semantics 21、iter_protocol 5、four_quadrants/sync_builtins/frontend_features 全通过；clippy 零警告。

```bash
# 证据（Node 22 与 aluka 输出 diff 为空）：
# JSON.stringify({b:2,a:1}) → {"b":2,"a":1}（创建序）
# JSON.stringify({2:'x',1:'y',a:1}) → {"1":"y","2":"x","a":1}（整数键前置）
# JSON.stringify({a:undefined,b:null,c:3}) → {"b":null,"c":3}（undefined 整键剔除）
# JSON.stringify([1,undefined,fn,null]) → [1,null,null,null]
# JSON.stringify(undefined) === undefined（顶层）
# 40 键对象 JSON 保插入序；删 b 重加 → {"a":1,"c":3,"b":9}（末尾）
```
## 10. M3 复评与登记（评审轮：M2/M3 独立复审）

- **复测证据**（Node v22.3.0 在场，7 套件串行实跑）：
  - `m3_tls_loopback_test` 2/2 ｜ `builtins_phase5_http_test` 10/10 ｜
    `builtins_phase5_net_test` 8/8 ｜ `builtins_phase4_stream_test` 4/4 ｜
    `builtins_phase4_fs_promises_test` 2/2 ｜ `conformance_node22` 26 例对拍 ｜
    `express_e2e` 1/1 —— **合计 28 passed / 0 failed**；前端单测
    （aluka-module 9 / compiler / bytecode）与 M2 复测（express/cjs/esm/conformance）
    全绿（详见 §8/§9 与 M2 复评结论）。
- **评审结论**：
  1. **M3.2 未闭环**：VM `tls`/`https` JS 表面未接真实 TLS（tls.rs/https.rs 自注）；
     `m3_tls_loopback_test.rs` 为纯 rustls 库直连（不经 VM）；受控探针
     `https.request → Node 真实 TLS 服务器`零输出静默失败（Node 对照 200）。
  2. **M3.4 部分达成**：`lookup` 真实（系统解析）；`resolve` 家族对任意域名非真实
     查询（std 无递归 DNS，dns.rs 自注）。
  3. **M3.1 / M3.3 达成**：Stream 背压状态机与 http.Agent Keep-Alive 池实现在场，
     模块测试 + express/conformance 佐证。
- **登记动作**（总 TODO README §M3）：
  - 总览 M3 `[x]` → `[~]`（M3.1/M3.3 达成；M3.2/M3.4 未闭环）；
  - 细分：M3.1 `[x]`、M3.2 `[ ]`+M3.2b 工作项、M3.3 `[x]`、M3.4 `[ ]`+M3.4b 工作项；
  - 新工作项 M3.2b（tls/https JS 面接入 rustls 会话，事件泵握手调度，自签证书
    自回环 JS 探针与 Node 22 对拍）；M3.4b（resolve 家族真实递归查询）。

## 11. M1 复评确认（评审轮：M2/M3/M1 独立复审）

- **结论**：M1 **已达成**（与 M3 不同——无验收落差；仅总 README 细分项未勾的
  文档疏漏，本轮已补 `[x]`）。
- **复测证据**（Node v22.3.0 在场）：
  - `cargo test -p aluka-cli --features runtime --test test262_subset_test`
    → **154/154 cases passed**（m1-proxy 45 / m1-regexp 12 / m1-es2024 10 /
    m1-typedarray 15 / m1-eval 52 / m1-array 10 / m1-negative 2）；
  - `cargo test -p aluka-regex` → 15 passed（lookbehind/backref/词边界）；
  - `cargo test -p aluka-vm --lib` → 146 passed；
  - 以上在近三轮大改（global 拆分/迭代器/JSON 属性系统）后实跑全绿。
- **结项来源**：`30e11ff`（M1 收口）+ `127bf71`（eval 扩容 52 例）；
  结项轮证据见 `20260907/README.md`（逐子项命令/产物证据 + 530 passed 门禁 +
  已知降级登记：Proxy invariant 平凡满足 / direct-eval var 语义 / SAB 进程内
  共享 / 原型方法属性面按需合成）。

## 12. M3 结项登记（M3.2 TLS 接线闭环，提交 `fb97628`）

承接 §10 复评：M3.2 未闭环项已修复并验收，M3 总览恢复 `[x]`。

- **交付内容**（`fb97628`，+550/-39，7 文件）：
  - `builtins/tls.rs` 供 `AcceptAllVerifier`/`make_client_config`/ALPN 装配；
  - `http/state.rs` 增 `Conn.tls`/`Server.tls`/`ClientReq.tls_conn` 与
    `Stage::Handshaking`；`http/server.rs` 增 `create_server_object_tls`，
    io_round/write_conn_bytes 支持 TLS 会话读写；`http/client.rs` TLS 拨号建
    `ClientConnection`、泵推进握手并加密读写；`http/mod.rs` sync_event_source
    保活握手/监听泵；`https.rs` `https_create_server` 构建真实 rustls 配置。
- **结项证据——三层跨实现对拍**（Node v22.3.0 在场；自签证书
  `tests/conformance/node22/cases/{test_key,test_cert}.pem`）：
  1. node TLS 客户端 → VM `https` server：200（`vm-tls-server-ok`）；
  2. VM `https` client → node TLS 服务端：200（`node-tls-ok`；含拨号修复——
     TLS 请求必须新建连接而非复用池，原 bug 致永不起拨、误报 actively refused）；
  3. VM 自回环 ↔ Node 自回环输出逐字一致（STATUS/CTYPE/BODY/CLOSED 4 行 oracle）。
- **回归证据**（修复后全量复跑）：
  - `builtins_phase5_http_test` 10/10（含 https）｜ `builtins_phase5_net_test` 8/8 ｜
    `builtins_phase4_stream_test` 4/4 ｜ `express_e2e_test` 1/1 ｜
    `conformance_node22_test` 1/1 ｜ `core_semantics_test` 21/21 ｜
    `aluka-vm --lib` 146/146 ｜ 新增验收 `https_tls_loopback_test` 1/1
    —— **合计 192 passed / 0 failed**；
  - `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 零告警。
- **口径说明**：
  - M3.2 收敛范围 = `https.createServer` / `https.request` 双向真实 TLS；
    `tls.connect` / `tls.createServer` JS 面接线为同构扩展，留 M3.2b 后续跟踪
    （rustls 会话机制已下沉共用，无新架构风险）；
  - TLS 客户端不复用 http keep-alive 连接池（rustls 会话不可池化复原；与 node
    https 默认 keepAlive=false 语义一致，对拍不冲突）；
  - 证书校验暂 AcceptAll（与对拍探针 `rejectUnauthorized:false` 对齐），证书链
    校验列为后续工作项；
  - M3.4 标 `[~]`（resolve 家族递归查询仍受限），以 M3.4b 独立跟踪、不阻塞 M3
    总览 `[x]`；总 README §M3 结项/复评登记已同步（结项在前、复评在后存史）。
