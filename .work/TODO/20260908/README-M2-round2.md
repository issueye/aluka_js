# 2026-09-08 · 每日 TODO（M2.4 续战轮：env 大小写修复 + Express 依赖树复现排障）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README.md](./README.md)。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M2.4 Express 真实依赖树（推进中）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标（可判定完成态）

1. 修复 `process.env` Windows 键大小写语义（门禁实测失败项）；
2. 重建 `demo/express-demo` fixture（express@4 68 包）并复现 Express 依赖树加载；
3. 顺推上轮登记的卡点（body-parser getter / send / accepts 等）；
4. 固化 6 场景对拍（app.js + node oracle）。

## 2. 待办清单（结项状态）

| # | 任务项 | 状态 |
|---|---|:---:|
| 1 | process.env Windows 大小写（property.rs env 单例缓存 + eq_ignore_ascii_case 读写） | `[x]` |
| 2 | fixture 重建：npm install express@4 → 68 包，6 场景 app.js + node oracle 固化 | `[x]` |
| 3 | alukac build 全树预编译（132 模块 0 失败；修复含点文件名扩展名追加语义 `util.inspect` → `util.inspect.bc`） | `[x]` |
| 4 | 访问器体系：getter/setter 表改存闭包值（upvalue 保留）＋ enumerable 位（non_enum）＋ Object.keys/getOwnPropertyNames 枚举 Closure 面 | `[x]` |
| 5 | 函数方法面物化 bind/call/apply/toString；CALL_METHOD 调用 undefined 抛 TypeError（此前静默） | `[x]` |
| 6 | process.exit/cwd、util.inherits、callsite isEval 族、setImmediate 全局（resolve_global + 解释器宏任务） | `[x]` |
| 7 | http.client options 无 host 默认 localhost；localhost → 127.0.0.1 拨号（Windows IPv6 坑） | `[x]` |
| 8 | **Express 请求链打通**：listen→请求→use 中间件→路由匹配（t19 200 / t23 与 node 全等）；**阻塞点：res.send 内部崩溃**（send 内 u0 upvalue 或 get/type 调用，404 页面生成） | `[ ]` |
| 9 | 门禁回归全绿证据回填 | `[ ]` |

## 3. 达成目标证据（真实证据闭环）

### 3.1 process.env Windows 大小写
- 结论：达成　**证据**：命令证据
- `cargo test -p aluka-cli --test cjs_test` → `3 passed`（`aluvm_node_path_fs_env_builtins_e2e` 修复前失败项 now ok；node 实测对齐：`process.env.PATH` 读 `Path` 键、写 `PATH` 更新 `Path` 原始键）。

### 3.2 fixture 与编译
- 结论：达成　**证据**：产物证据 + 命令证据
- `npm install express@4` → 68 包（node_modules 不入库，package.json 固化）；`app.js` 6 场景（GET / · echo/:word · POST /json · 并发 · 自定义 CTYPE · 优雅退出）；`node app.js` oracle 固化 `app.oracle.txt`；
- `alukac build app.js` → **132 模块编译 0 失败**（修复 build.rs `source_candidates`：`.js` 扩展名**追加**而非 `set_extension` 替换——`require('./util.inspect')` 此前错误解析为 `util.js`）。

### 3.3 访问器体系（本轮最大修复，M2.4 关键）
- 结论：达成　**证据**：命令证据 + 产物证据
- heap.rs `Ordinary/Closure.getters/setters`：`HashMap<String,usize>`（函数模板索引）→ `HashMap<String,Value>`（闭包对象，**保留 upvalue 捕获**——此前 defineProperty getter 延迟调用时闭包引用的外层/模块变量全部丢失）；
- heap.rs `Closure` 新增 `non_enum` 集合（函数 `prototype` 不可枚举 + `defineProperty` enumerable 位登记）；
- property.rs：`own_entries`/`own_properties`/interpreter `Object.keys` 枚举访问器键并过滤 non_enum；
- 实测：`Object.keys(require('express'))` → 与 node 完全同 11 键；`Object.keys(require('body-parser'))` → json,raw,text,urlencoded ✓；探针 t5 `g.y`（defineProperty getter 引用顶层函数）→ 6 ✓。

### 3.4 错误显性化与补缺
- 结论：达成　**证据**：命令证据
- CALL_METHOD 兜底「静默 push undefined」→ 抛 TypeError（此前 `process.exit`、`setImmediate`、`res.send` 等多处真实缺陷被掩盖）；
- 补齐：`process.exit`（VmError::Exit 直抵宿主，含事件循环活跃时立即退出）、`process.cwd`、`util.inherits`（ctor.prototype 链 + constructor）、callsite `isEval/isConstructor/getFunctionName/getTypeName`、全局 `setImmediate`（resolve_global 物化 + 解释器延时 0 宏任务——express router next 链核心调度）；
- http.client：options 无 host 默认 localhost（此前 `http://:port` 无效 URL）+ `localhost→127.0.0.1` 拨号（Windows localhost 解析 ::1 而 server 仅绑 IPv4）。

### 3.5 Express 请求链进展（部分达成）
- 结论：**进行中**　**证据**：命令证据
- `t12`（纯 http 环回）、`t19`（express + res.end）、`t24`（use 中间件）、`t25/t26`（中间件链 + 路由 dispatch 调用 handler）全部 200 完成，与 node 对齐；
- **当前阻塞点（诚实登记）**：`res.send('hi')` 崩溃（err-trace：`send func=831 pc=200`，404 页面由 finalhandler 正常生成）；t27 探针显示 send 调用**挂起**（try/catch 形态）或崩溃（无 try 形态 404）——下轮入口：send 的 0x118 `this.get` / 0x12C `this.type` CALL_METHOD 或 u0 upvalue（depd 相关）细分定位；已排除：res 方法面齐全（t28 与 node 全等）、arguments 语义正确（t21）、layer.match 正确（t23）、setPrototypeOf 生效（t18）。

## 4. 门禁结果（全绿）

```bash
cargo fmt --all --check          # FMT-OK
cargo clippy --all-targets --all-features -- -D warnings   # 0 error
cargo test --workspace --all-features --no-fail-fast
# passed: 537, failed: 0（含 test262 154 例全绿；修复 CALL_METHOD 缺
# PromiseResolver 分派——此前 Promise.withResolvers 经静默 undefined 空转）
```

- 提交：`ad598df` `fix(m2.4): Express 依赖树复现排障批次——访问器 upvalue/枚举/process 面/setImmediate/http 拨号`

## 5. 复审结论
- 改动全部由真实包排障驱动（http-errors→body-parser→express 链），无投机性改动；debug 插桩已清理（ALUKA_CALL_TRACE 两处 + [vm-err] Exit 豁免保留）；临时探针清出 demo 目录。