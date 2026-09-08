# 2026-09-09 · 每日 TODO（M2.4 状态核实 + 6 场景对拍续战轮）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README.md](./README.md)。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M2.4 Express 真实依赖树（结项推进中）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标（可判定完成态）

1. 基于最新代码 `3c0f28f` 重建全部二进制与 `demo/express-demo` 全树 .bc 产物（不信任旧 target 产物）；
2. 实测 app.js 在 aluka 上的真实运行状态，与 oracle 逐项对拍；
3. 定位并修复阻塞（09-09 轮登记：response.js 加载崩溃、POST body 空）；
4. 修复后跑通并回填门禁证据。

## 2. 待办清单（结项状态）

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | 重建 aluka/alukac/aluvm 二进制 | `[x]` | `cargo build` 全量成功；`aluka.exe` 时间戳更新 |
| 2 | alukac 全树重编译 app.js 依赖 | `[x]` | **132 模块编译 0 失败**（命令证据） |
| 3 | **根因定位：String/Number/Boolean/Date 构造器 prototype 全指向 `vm.object_prototype` 空对象**——方法面单例（str_proto 等）无人引用，真实包存槽 `String.prototype.slice/indexOf` 全 undefined，get-intrinsic 绑定链把 undefined 当函数调用即崩 "undefined is not a function"（express 全树加载与 response.js 崩溃的**同一根因**） | `[x]` | global_fns.rs 修复：4 个 ctor prototype → surface 方法面单例（Date 独立普通原型）；差分探针 R2=1 / R5=1 与 Node 全等（命令证据） |
| 4 | **09-09 遗留门禁回归 ①**：`http_write_head_and_204_matches_go` 失败——测试请求 `/404` 但路由为 `/404/404`（曾依赖 09-09 修复前的 client path 翻倍 bug 才输出 404） | `[x]` | 测试夹具路径修正 `/404`→`/404/404`、`/empty`→`/empty/empty`；phase5_http 10/10 通过 |
| 5 | **09-09 遗留门禁回归 ②**：`dgram_loopback_send_receive_e2e_matches_go` 失败——CALL_METHOD bind 特判（09-09 #4）无 receiver 函数守卫，把 `dgram.Socket.bind()` 实例方法误当 `Function.prototype.bind`（绑定函数返回、事件源未激活） | `[x]` | worktree ad598df 对拍：该测试 ad598df 通过 / HEAD 失败（回归铁证）；interpreter.rs bind 特判加函数对象守卫后 8/8 通过 |
| 6 | 6 场景对拍（app.js http 形态：fetch 属 M4.1 不在 M2.4 验收范围，改 http 环回） | `[ ]` | **GET / 与 ECHO 场景 200 全等 oracle**；POST /json 仍崩：body-parser→raw-body→iconv-lite `getDecoder` 链（09-09 遗留第二阻塞点，下轮入口） |
| 7 | 门禁全绿 | `[x]` | fmt-OK / clippy-0 / `cargo test --workspace --all-features` 全绿（0 failed，含 test262 154 例） |

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                          # FMT-OK（顺带修正 09-09 批 lexer/sqlite/surface/typed_array 格式遗留与 sqlite BOM）
cargo clippy --all-targets --all-features -- -D warnings   # 0 error（修复 lexer 2 处 extend(chars) lint）
cargo test --workspace --all-features           # 全绿 0 failed（phase5_http 10、phase5_net 8、146+33 单测等）
```

## 4. 遗留问题（下轮入口）

1. **POST /json 场景（第二轮进展）**：
   - 已修复三链：iconv-lite getDecoder 崩（编译器 `collect_ident_uses` 漏 MultiVarDecl 初始化器收集 → 模块变量退化为 LoadGlobal）；raw-body invokeCallback 崩（upvalue 需求未上抛——孙级函数声明引用祖父层名时父函数不自引用即断链，补 `Stmt::Function` 递归收集）；StringDecoder.write/end 对 Buffer 参数按字节提取（`extract_bytes`）
   - **剩余**：raw-body readStream 的 `received += chunk.length` 偶发写丢（插桩改变代码布局后行为变化——疑似编译器局部槽位/upvalue 编号布局敏感缺陷），以及 http-errors `createError` 在该链上返回 undefined 的次生现象；GET/ECHO/并发/CTYPE/优雅退出 5 场景已全绿
2. S5/S10 语义：`Object.getPrototypeOf('') === String.prototype` 仍 false（非本轮 express 阻塞）
3. `express_e2e_test.rs` 固化测试尚未创建（待 6 场景全绿后补）
