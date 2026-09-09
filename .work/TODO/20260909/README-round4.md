# 2026-09-09 · 每日 TODO（M4 收尾轮：M4.1 Request/重定向 + M4.3 定时器/Socket abort + M4.4 EventTarget/FormData）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round3.md](./README-round3.md)。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M4 现代 Web API 对齐（收尾完成）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标（可判定完成态）——全部达成

1. **M4.1 剩余** ✅：`Request` 全局构造器（url/method/headers/body/signal 继承 + init 覆盖
   + GET/HEAD body TypeError）；fetch(request) 直传；重定向三模式
   （follow ≤5 跳/manual/error→TypeError）；`Response.headers` 小写展示面 +
   `Headers.get/has`（大小写不敏感）；`arrayBuffer()`/`Response.formData()` 实装；
   **chunked 传输解码**（Node http server 默认分帧——旧实现 body 混入 `d\r\n`/`0\r\n` 帧头）；
   `https://` 显式 rejected TypeError（不再静默明文连接）。
2. **M4.3 剩余** ✅：`setTimeout/setInterval/setImmediate` options.signal——abort 时
   等价 clearTimeout（经 AbortSignal 'abort' 监听通道 `timers.signalClear`）；
   `timers/promises.setTimeout` signal 已 abort → 定时器清除 + reason 拒绝；
   net `connect/listen` options.signal——abort 销毁 socket / 关停 server
   （`net.signalDestroy`/`net.signalCloseServer`）。
3. **M4.4** ✅：`EventTarget`（addEventListener/removeEventListener/dispatchEvent +
   target 注入）/ `CustomEvent`（type/detail）全局构造器；`FormData` 全方法面
   （append/set[原位置替换]/get/getAll/has/delete/entries/keys/values/forEach）
   + multipart 编码 + urlencoded/multipart 解析（Response.formData）。

## 2. 待办清单（全部完成）

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | M4.1 Request 构造器 + fetch(request) + 重定向三模式 | `[x]` | case 23 差分（conformance src 形态 + phase9 双对拍）：`follow: 200 true reached-final` / `manual: 302 false` / `error-mode: TypeError` 与 Node 逐字一致 |
| 2 | M4.1 Response.headers/arrayBuffer/https 拒绝/chunked 解码 | `[x]` | `headers-get: text/plain | missing: null` / `headers-has: false true` / `https-reject: TypeError`；chunked 修复后 `one: 200 reached-final`（旧实现 `d\r\nreached-final\r\n0\r\n`） |
| 3 | M4.3 timers signal 选项 | `[x]` | case 24：`timer-aborted: true` / `immediate-aborted: true` / `timer-normal: true` 与 Node 逐字一致 |
| 4 | M4.3 net connect/listen signal 选项 | `[x]` | `net.signalDestroy`/`signalCloseServer` 分派 + AbortSignal 监听挂接（探针验证 abort→destroy 路径；用例形态见 phase9 附录） |
| 5 | M4.4 EventTarget/CustomEvent 全局 | `[x]` | case 22：`dispatch-ret: true` / `log: ping:42|second` / `ce.type: ping ce.detail: 42` 与 Node 逐字一致 |
| 6 | M4.4 FormData + multipart + Response.formData | `[x]` | case 22：`fd.get/getAll/forEach/after-delete` 全一致（set 原位置替换对齐 Node 迭代序）；multipart 编码边界 + urlencoded 解析实装 |
| 7 | 差分用例固化 | `[x]` | `22-m4-web-standards.cjs` / `24-m4-timers-signal.cjs`（conformance，bc 模式）；`builtins_phase9_m4_test.rs`（fetch 网络面，src 模式 + Node 实时双对拍） |
| 8 | 门禁全绿 | `[x]` | 见下 |

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                            # FMT-OK（exit=0）
cargo clippy --all-targets --all-features -- -D warnings   # 0 error 0 warning（exit=0）
cargo test --workspace --all-features              # 全绿 exit=0（0 failed；
                                                   # conformance 24 用例含新增 22/24 全过；
                                                   # phase9 fetch 差分 1 passed）
```

## 4. 差分对拍证据汇总

- **case 22**（Web 标准）：Aluka ≡ Node 22 逐字一致（EventTarget 派发/CustomEvent/
  FormData 全方法）
- **case 24**（timers signal）：逐字一致（abort 取消 + 未 abort 正常触发）
- **phase9**（fetch 进阶，src 模式实时双对拍）：`follow/manual/headers-get/headers-has/
  https-reject/error-mode/request-obj` 七行逐字一致
- **chunked 解码**：修复前 `d\r\nreached-final\r\n0\r\n` → 修复后 `reached-final`（与
  Node 逐字一致）

## 5. 发现的引擎级既有缺陷（超出本轮 M4 范畴，登记下轮）

1. **cluster + aluvm bc 模式 + fetch ≥2 TCP 连接挂死**（P0）：src 模式同代码正常；
   外部 server + bc 正常；单连接 bc 正常——最小复现 `double_bc.cjs`（cluster +
   双 fetch）。疑 bc VM 主进程 fetch 同步阻塞期间与 cluster 子进程泵线程死锁。
   M4 差分固化策略：conformance 只收 bc 稳定用例（22/24 单连接），fetch 网络面
   走 phase9 src 模式双对拍；该缺陷修复后可回迁。
2. **promise.then 链返回值展平缺失**（P1）：`then` 返回 receiver 自身而非衍生
   promise（interpreter.rs:2547 附近）——链式 `.then(r => fetch(...)).then(r => ...)`
   第二环 r 为 undefined。async/await 形态正常。案例：m4_redirect.cjs 初版。
3. **`process.exit(0)` 输出"执行错误"且 exit=1**（上轮遗留 #3，未修）。
