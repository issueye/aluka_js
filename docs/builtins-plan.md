# 内置库补齐计划（Node 兼容层 · M2）

> 状态：计划（2026-09-04 定稿）
> 关联：`rust-reimplementation-devplan.md`（M2「内置库建设」）、
> `.github/workflows/ci.yml`（Go oracle 对拍门禁）、`node22-conformance-review.md`
> 参考：Go 侧 `aluka_g/internal/builtin/registry.go`（60 个 `RegisterBuiltin`）

## 1. 目标与边界

**目标**：aluka 的 Node 兼容内置库从「最小可跑」走向「真实脚本可写」——
按使用频率由轻到重分批补齐，**每一批以 Go Oracle 逐字对拍 + e2e 回归固化**
为验收闭环（Tier 0 纪律：Go 版是唯一 oracle）。

**边界**：
- 内置实现分两类：**纯状态**（path/os/url 等，无回调）与**回调承载**（events/stream/fs 异步），
  后者依赖已就位的微任务队列（M2 已完成）；
- 不做 Web-only 面（DOM/WebSocket）与浏览器 API——`aluka-webapi` 另轨；
- 重型模块（http2/cluster/dgram/x509）以「能为 conformance 用例提供语义等价
  输出」为验收，不追求与 Node 逐字节协议兼容（那些用例 Go 侧同样不达）。

## 2. 现状盘点（已实现矩阵，实证：`aluka-vm/src/interpreter.rs` 拦截点）

| 域 | 已实现 | 载体 |
|---|---|---|
| console | `log`（数组/对象格式化） | CALL_METHOD 拦截 |
| Math | `sqrt` | 同上 |
| 构造器 | `Error/Array/Object/Map/Promise/URL`（`new` 与 `instanceof` 语义） | NativeCtor 分派 |
| process | `argv`（CLI 注入）/ `nextTick` / `env` | 单例 + 拦截 |
| fs | `readFileSync` / `writeFileSync` / `existsSync` | 同上 |
| path | `join` / `basename`（含去扩展名二参）/ `dirname` / `extname` / `resolve` | 同上 |
| os | `platform` / `homedir` / `tmpdir` / `EOL` | 同上 |
| 全局函数 | `setTimeout`（宏任务排队）/ `queueMicrotask` | NativeFn + Call |
| 数组原型 | `push/pop/join/map/filter/find/some/forEach/reduce/reduceRight/slice/sort/toSorted/toReversed/toSpliced/with` | Array.prototype 属性 + 拦截 |
| 字符串原型 | `isWellFormed` / `toWellFormed` | 拦截 |
| Object / Map | `groupBy` / `hasOwn` / `create` / Map `get` | 拦截 + 变体 |
| Promise | `resolve` / `withResolvers` / `then` / `catch`（微任务调度） | 拦截 + 变体 |
| RegExp | `exec` / `test`（aluka-regex 引擎） | 拦截 |
| 生成器 | `next()` 驱动（fromAsync 消费） | 拦截 |

已有 8 类 40+ 能力点，全部经 Go Oracle e2e 对拍固化（`aluka-cli/tests/cjs_test.rs` 3 组）。

## 3. 差距全景（对齐 Go `registry.go` 的 60 模块，四分层）

| 层 | 模块 | Go 实现参照 | 性质 |
|---|---|---|---|
| **T1 轻量纯状态** | path/posix、path/win32、constants、querystring、string_decoder | nodeos/nodeutil/nodestream 子集 | 纯函数/常量，无回调，低依赖 |
| **T2 文件与进程** | fs 补齐（readdirSync/statSync/mkdirSync/rmSync…）、os 补齐（cpus/arch/release…）、util（format/inspect/is*）、util/types、assert | nodefs/nodeos/nodeutil | 同步为主 |
| **T3 事件与流** | events（EventEmitter）、stream/readable/writable、timers（setInterval/clearTimeout）、v8/getHeapStatistics、perf_hooks | nodeevents/nodestream/nodetimers/nodediag | 回调承载（依赖微任务/宏任务基建 ✓） |
| **T4 网络与重件** | http/https/net/tls/dns、url 深化（parse/format）、crypto、zlib、inspector、vm、worker_threads | nodehttp/nodenet/nodecrypto/nodevm | 大件，验收=conformance 语义等价 |

> 备注：registry.go 中 `node:sqlite`（nodesqlite）为演示性注册，Go 侧亦非
> 完整实现，Tier 4 排后；`node:test`（nodetest）是 15-test-runner 用例的
> 依赖，列 Tier 3.5 由 T3 完成后跟组。

## 4. 分批路线（每批：实现 → Go Oracle e2e 对拍 → 固化 cjs_test → 门禁）

### Phase 1（T1，≈2-3 次会话）
- `path/posix`、`path/win32`（复用 path 解析内核，分隔符参数化）；
- `querystring`（parse/stringify——纯字符串，conformance 高频）；
- `constants`（信号/错误码常量表）；
- `string_decoder`（StringDecoder——UTF-8 增量解码，std 可支撑）。
- 验收：每模块 ≥1 e2e 用例与 Go 逐字对拍。

### Phase 2（T2，≈3-4 次会话）
- `fs` 补齐同步族：`readdirSync/statSync/mkdirSync/rmSync/readFileSync(enc)`；
- `os` 补齐：`cpus/arch/release/type/uptime/userInfo`；
- `util`：`format/inspect（基础）/isArray/isString/…`；
- `assert`：`ok/equal/strictEqual/throws`；
- 验收：真实脚本风格 e2e（文件树操作 + 断言链）对拍。

### Phase 3（T3）
- [x] `buffer` 基础：`Buffer` 构造器与类对象、`SlowBuffer`、`kMaxLength`、`isUtf8`、`isAscii`、静态方法（`from`/`alloc`/`allocUnsafe`/`isBuffer`/`byteLength`/`isEncoding`/`concat`/`compare`）、实例方法（`toString`/`slice`/`toJSON`/`equals`）；
- [x] `timers` 与 `timers/promises`：定时器族导出，`setTimeout`/`setImmediate` Promise 化支持供 async 脚本原生 `await`；
- [x] `perf_hooks`：`performance.now`（单调高精度毫秒）、`timeOrigin`、`mark`、`measure`、`getEntries*`、`clearMarks`（消除 conformance 13 缺口）；
- [x] `v8`：`getHeapStatistics`（14 个 Node 规范统计键）、`cachedDataVersionTag`、`serialize`/`deserialize`（消除 conformance 12 缺口）；
- 验收：`crates/aluka-cli/tests/builtins_phase3_test.rs` 4 组 e2e 用例与 Go 逐字对拍全绿固化。

### Phase 4（核心事件、流与异步增强，四特工多代理并行开发完成）
- [x] `events`：`EventEmitter` 类与实例完整生命周期（`on`/`once`/`emit`/`off`/`listenerCount` 等）；
- [x] `stream` 家族：`Readable`/`Writable` 管道流转、`stream/promises`（`finished`/`pipeline`）、`stream/consumers`（`text`/`json`/`buffer`）；
- [x] `fs/promises`：`readFile`/`writeFile`/`readdir`/`stat`/`mkdir`/`rm` 原生 Promise 化支持 `await`；
- [x] `assert/strict`：严格断言模式与函数直调支持；
- [x] `sys`：历史兼容别名模块，透明转发 `util`（符合 DEP0140 规范）；
- 验收：4 组端到端测试套件（13 个对拍测试）全部 100% 绿通固化。

### Phase 5（网络通信协议栈与重型大件）
- `crypto`（hash/randomBytes）、`zlib`（deflate/inflate）、`http/https/net`（TCP/HTTP 最小，验收=Go conformance 08/10 语义等价）、`vm`（runInContext 近似）。
- 验收：conformance 缺口清单逐项刷新（当前 4/17 语义类全过，目标 ≥8 项有 Go 输出）。

## 5. 验收与门禁（贯穿所有 Phase）

1. **Go Oracle 逐字对拍**：每能力点一个 e2e 用例（`cjs_test.rs` + 单入口 fallback 已就绪）；
2. **conformance 清单刷新**：`node22-conformance-review.md` 的缺口表随 Phase 更新；
3. **全量回归**：`cargo test`（93+ 项）+ clippy `-D warnings` 0 错误 + fmt；
4. **性能纪律**：新增回调承载（T3）时以 fib30 基线（388.3ms）回归确认无退化。

## 6. 风险与已知缺口

| 风险 | 说明 | 对策 |
|---|---|---|
| 平台差异 | path/os 输出按平台（Win vs Unix）不同 | 与 Go filepath 同口径（std::path 平台语义）+ 对拍用例按平台参数化 |
| 异步回调深度 | T3 的 EventEmitter/stream 回调链可能触发深层微任务 | 微任务队列已有循环执行；e2e 用例覆盖嵌套 emit |
| Buffer 缺失 | 二进制读写（readFileSync 返回 Buffer） | Phase 2 引入 `Buffer` 构造器（等长 Uint8Array-like）或先返回字符串（对齐 Go 现状——Go 的 readFileSync 返回？按实测对齐） |
| conformance 数字 | 9 项 Node 内置用例 Go 侧亦失败（空输出对拍无意义） | 以「Go 有输出的用例」为推进指标，不做空对空 |
| 前端在途 | ESM 解析由 A2 轨推进中 | 内置库不依赖 ESM；两线并行 |

## 7. 挂接与记录

- 每 Phase 完成：今日 TODO 增条（内含 e2e 对拍命令证据）；
- 计划修订：Phase 边界随 conformance 清单刷新调整，回写本文档。