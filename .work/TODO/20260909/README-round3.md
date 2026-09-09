# 2026-09-09 · 每日 TODO（M5 评审缺陷修复轮）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round2.md](./README-round2.md)。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M5 评审修复（结项推进中）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标（可判定完成态）

针对 M5 评审发现的已实证缺陷逐一修复并以差分对拍验证（修复文件：
`runner.rs` / `context.rs` / `state.rs` / `worker_threads.rs` / `net.rs` / `express_e2e_test.rs`）。

## 2. 待办清单（全部完成）

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | 评审报告产出（P0×2 + P1×2 + P2 + 中级若干，探针差分实证） | `[x]` | 本会话评审记录 |
| 2 | **P0-1 并发测试归属修复**：① start 阶段包 `scoped_current(state_id)`；② t/t.assert 挂 `_stateId`，全部 ctx 方法（assert 家族/plan/skip/todo/test）改 receiver 绑定取状态（`with_state_mut`）；③ 顺序路径 `invoke_with_state` 微任务+宏任务交替驱动（旧实现只排微任务，`await setTimeout` 挂起被假判 Done） | `[x]` | 差分探针 `conc_probe2.cjs`（await 后 plan(2) 断言 1 次）：Node=FAIL/Aluka=FAIL 语义一致；`conc_probe.cjs`（并发批 await 后 plan/2 断言/子测试）：PASS sub + PLAN 4 4 0 全部正确归属 |
| 3 | **P0-2 worker exit 双发修复**：`pump_real_workers` Exit 分支删 `if is_exit {continue}` 跳过存活检查的错误设计，改 Exit 复查 REAL_WORKERS 存活 + 派发循环二次防护 | `[x]` | 用例 20 连跑 10 次 `abnormal-runs=0/10`（修复前 ~50% 双行）；与 Node 22 输出逐字一致（`main exit: 1 sq:400:start:t1` / `main exit2: 1`） |
| 4 | **P1-1 EADDRINUSE 修复**：`bind_shared_listener` 仅 cluster worker（`ALUKA_WORKER_ID` 环境标记）走 SO_REUSEADDR/REUSEPORT，普通进程回退 `TcpListener::bind` 独占 | `[x]` | 差分探针 `addrinuse.cjs`：普通进程双 listen 同端口触发 'error'（Node=`ERR: EADDRINUSE true`；Aluka=error 事件触发，e.code/message 载体为字符串系既有 Go 遗留，登记下轮）；用例 21 cluster 共享绑定仍逐字一致（`probe: 200 worker-ok`） |
| 5 | **P1-2 express fixture 固化**：app.js 源码固化进测试（`APP_JS` 常量），运行期以 `.e2e_app_<pid>.js` 唯一名落盘 demo/express-demo 根（build root=demo 根，node_modules 全镜像），结束后清理 | `[x]` | `cargo test -p aluka-cli --test express_e2e_test` → `1 passed; 0 failed`；fixture/产物清理后 `git status demo/` 干净 |
| 6 | **P2 settle 超时假通过**：120s 超时后 pending promise 用例判 `passed=false` + `test timed out after 120000ms awaiting promise`（并发批与顺序路径双修） | `[x]` | runner.rs 两处 timeout 分支（`settle_timeout` 标记 + `invoke_with_state` Err 上抛）；全量测试回归通过 |
| 7 | 门禁全绿 | `[x]` | 见下 |

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                            # FMT-OK（exit=0）
cargo clippy --all-targets --all-features -- -D warnings   # 0 error 0 warning（exit=0）
cargo test --workspace --all-features              # 全绿 exit=0（含 express_e2e 1 passed、
                                                   # conformance_node22 1 passed、phase6-8 全过、
                                                   # 146+33 单测等，0 failed）
```

## 4. 差分对拍证据汇总（修复后复验）

- `tests/conformance/node22/cases/20-m5-worker-threads.cjs`：Aluka ≡ Node 22 逐字一致 ×10 次
- `tests/conformance/node22/cases/21-m5-cluster-http.cjs`：Aluka ≡ Node 22 逐字一致（`probe: 200 worker-ok`）
- 并发归属探针：await 后 plan 短断言 FAIL 语义与 Node 一致；await 后子测试 PASS 派发恢复
- EADDRINUSE：普通进程端口冲突 'error' 事件恢复触发（Node oracle：EADDRINUSE）

## 5. 遗留问题（下轮入口）

1. **net listen 失败错误载体**：'error' 事件实参是 OS 字符串而非 Error 对象（`e.code` undefined；Node=Error+code EADDRINUSE）——Go 系遗留形态，建议统一 Error 实例 + code 属性。
2. **worker 嵌套**：worker 线程内 `new Worker` 未 `install_worker_entry` → 回落伪 worker（Node 支持真嵌套物理线程）。
3. **`process.exit(0)` 语义**：正常退出码被当"执行错误"输出且 exit=1（Node=0 静默）——评审发现，未在本轮范围。
4. **fetch https:// 明文**：`https://` 前缀剥离后走 TcpStream（无 TLS）——建议显式 rejected TypeError 兜底（M4 范畴）。
5. **`.bc` 优先启发式**（`resolve_worker_input`）：`app.js → app.bc` 同目录碰撞无防护。
6. **thread_local 跨 Vm 撞键**：堆句柄每 Vm 从 0 起，同线程顺序复用进程时旧表残留可撞键（当前主流程单 Vm 每进程，风险潜伏）。
