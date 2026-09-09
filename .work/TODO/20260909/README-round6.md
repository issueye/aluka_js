# 2026-09-09 · 每日 TODO（M5 续轮 round6：M5.2 P0——bc 模式 cluster + fetch ≥2 挂死）

> 总 TODO 见 [../README.md](../README.md)；上一轮（M5.3 闭环）见 [./README-round5.md](./README-round5.md)。

**当前里程碑**：M5 多线程并发与系统级扩展（遗留清理）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

## 1. 本轮目标（可判定完成态）

按评审建议顺序第二项——**M5.2 P0**（bc 模式 cluster + fetch ≥2 TCP 连接挂死，
round4 §5 登记）：

1. 重建最小复现（bc 编译 cluster + 双并发 fetch，src 模式对照）；
2. 定位死锁根因（疑 bc 主进程 fetch 同步泵与 cluster 子进程泵/事件源互卡）；
3. 修复后：conformance 22/24 中绕行用例回迁 bc 模式验证（round4 提到的
   「修复后可把 conformance 22/24 中绕行用例回迁」——含 21-m5 多连接化）；
4. 门禁全绿 + 登记（P0 关闭后 M5.2 复核：IPC 面降级仍在，总览维持 [~]）。

## 2. 待办清单

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | round6 登记 + 最小复现重建 | `[x]` | double-fetch/observe/wlog 系列探针 |
| 2 | 死锁根因定位 | `[x]` | 见 §2b |
| 3 | 修复 + bc 对拍回迁 | `[x]` | 20s/请求 → 5-13ms;conformance/phase9 绿 |
| 4 | 门禁三连 + 登记 | `[x]` | 见 §3 |

## 2b. 根因与修复（P0 关闭）

**表象**（round4 登记「bc 模式 cluster + fetch ≥2 连接挂死」）：实际为
**每次请求固定 ~10s 延迟后成功**（非死锁）；src/bc、单/双 worker、共享/
独占监听、同/跨进程全部复现——与 cluster 无关。

**根因链**（多轮对照实验收敛）：
1. `fetch` 客户端（`do_sync_http_request`）发 `Connection: close` 后
   **循环阻塞读直到 EOF**，socket 读超时 10s；
2. aluka `http` server 响应后按 keep-alive **保持连接不关闭**（Node 对
   `Connection: close` 请求会在响应后关闭；`finalize_response` 只
   `mark_conn_idle`）→ fetch 空等 EOF 到 10s 读超时；
3. 读超时 break 时响应字节早已完整在缓冲 → 解析出 200——所以「10s 后
   成功」而非报错；curl/裸客户端同样受影响（早期观测 curl 0.1s 异常退出
   为同类连接语义问题的另一表现，修复后 curl 0.02s 正常）；
4. round4 误判为「cluster+bc 多连接死锁」：实际并发双 fetch 各自空等
   10s 串行叠加，观测上呈挂死。

**修复**（fetch 侧，undici 语义——不依赖连接关闭判定响应完成）：
`do_sync_http_request` 读循环新增 `response_complete`：头部结束标记 +
（`Content-Length` 收满 | `Transfer-Encoding: chunked` 终止块到达）即
返回；无长度（close 定界）维持等 EOF/超时兜底。aluka http server 对
`Connection: close` 响应后关闭连接的 Node 语义另行登记跟踪。

**验证**：
- 并发双 fetch（cluster bc）：20024ms → **13ms**；单 worker 10s → 2ms；
  src 模式 5ms；curl → aluka server 0.02s；
- phase9（M4 fetch 网络对拍）71s → **1.1s** 全绿；phase5 http 10/10；
  conformance（--all-features）24 绿 2 invalid 通过；
- 全量 561 passed / 0 failed。

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                       # FMT-OK
cargo clippy --workspace --all-targets --all-features -- -D warnings  # 零警告
cargo test --workspace --all-features         # 561 passed / 0 failed
```

## 4. 提交

```bash
git commit -m "fix(m5.2): fetch 响应完成判定不依赖连接关闭——P0(10s/请求)关闭 + conformance 提速"
```

## 5. 遗留/下轮

- M5.1 结构化克隆 → M5.4 Timer Mock + CLI 运行器；
- M5.2 仍登记：IPC 面（worker.send/isConnected/isDead/exit code）降级、
  `Connection: close` 响应后关闭连接的 server 语义、net listen 错误载体
  字符串化；总览维持 [~]。
