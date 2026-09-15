# taskboard-demo · Aluka 运行时真实项目实测

一个**真实结构**的 Node.js 项目（存储层 + 业务层 + HTTP API + CLI + node:test 套件 +
真实 npm 依赖），用于对 Aluka 运行时做端到端实测，并与 **Node.js 22** 逐行对拍。

> 实测结论与运行时缺陷清单见 [`.work/TODO/20260915/README-round94.md`](../../.work/TODO/20260915/README-round94.md)。

## 结构

```
taskboard-demo/
├── e2e.js                    # 端到端脚本（真实 fs + 真实 HTTP + 真实 npm 依赖 + 并发）
├── src/
│   ├── config.js             # 配置装载（默认值 ← 文件 ← 环境变量）
│   ├── logger.js             # 结构化日志（字段排序保证输出稳定）
│   ├── errors.js             # 领域错误（AppError → Validation/NotFound/Conflict/…）
│   ├── store.js              # JSON 文件存储（原子落盘：临时文件 + rename）
│   ├── service.js            # 业务规则（校验 / 状态流转 / 去重 / 统计）
│   ├── http-server.js        # node:http + EventEmitter 路由层
│   └── cli.js                # CLI（add/list/done/rm/stats/serve）
├── test/
│   ├── store.test.js         # 存储层单测（真实文件 IO）
│   ├── service.test.js       # 业务规则单测（内存桩）
│   ├── api.test.js           # HTTP 集成测试（真实监听 + 往返）
│   └── helpers/e2e-lib.js    # e2e 公共工具
├── tools/                    # 运行时能力/缺陷探针（自包含，无相对依赖）
└── probe-*.js / pb-*.js      # 根目录复现脚本（入口须在项目根，见下「已知限制」）
```

## 运行

```bash
# 依赖（真实 npm registry）
npm install                       # 或 aluka npm install

# 端到端（确定性输出，可直接与 node 对拍）
node e2e.js                       # Node.js 22 oracle
aluka run e2e.js                  # Aluka

# 测试套件
node --test test/                 # Node
aluka test test/                  # Aluka（node:test 运行器）

# CLI
node src/cli.js add "写周报" --priority high --tags doc,daily
node src/cli.js stats
node src/cli.js serve --port 3000
```

## 实测状态（Aluka vs Node.js 22）

### ✅ 与 Node 逐字节一致

| 场景 | 证据 |
|---|---|
| 配置装载 / 结构化日志 / 字段排序 | `e2e.js` `### config` 段 |
| **真实 npm 依赖 `ms`**（`npm install` 后 `require('ms')`） | `e2e.js` `### dependency` 段 |
| JSON 文件存储（落盘 / 重载 / 原子 rename / 错误码） | `e2e.js` `### store` 段 |
| 业务规则（校验 / 状态流转 / 去重 / 统计 / 过滤排序） | `e2e.js` `### service` 段 |
| fs 同步族（含 `renameSync`/`unlinkSync`/`copyFileSync`/`appendFileSync`/`realpathSync` 与 ENOENT/EEXIST 错误对象） | `tools/cap-probe.js`、`tools/probe-rm.js` |
| 类方法内**箭头函数 `this`**、`super`、getter、继承 | `tools/probe-this.js`、`tools/probe-this2.js` |
| `node:http` 服务端/客户端往返、`EventEmitter`、`Buffer`、`crypto`、`Date`、`JSON` | `tools/cap-probe.js` |
| 项目自带测试套件（22 用例） | `node --test` 22/22（Oracle）；见下方限制 |

`e2e.js` 在 Aluka 上**前 32 行**与 Node 逐字节一致（config → dependency → store → service 全段）。

### ❌ 阻塞项（运行时缺陷，复现脚本已给出）

| # | 现象 | 复现 |
|---|---|---|
| 1 | 跨模块闭包/类调用误执行**另一模块同索引函数体**（顺序相关，潜伏型） | `pb-c.js`（失败）/ `pb-h.js`（同模块、顺序不同 → 通过） |
| 2 | HTTP 回调体不执行 → `server.listen()` 永不 resolve | `probe-http.js`（Aluka 挂起 / Node 正常） |
| 3 | 入口在子目录时跨目录 `require` 不可解析（`_ext` 扁平化） | `tools/probe-taskboard.js`、`aluka test test/` |

### ⚠️ 项目布局约束

**入口文件必须位于项目根**（`e2e.js` 在根）。若入口在 `tools/`、`scripts/` 等子目录，
其 `../src/*` 依赖会在运行期报 `Cannot find module`（限制 3）。


## 说明

- `e2e.js` 的输出**完全确定**（固定时钟、无绝对路径、无随机值、字段排序），
  因此可与 Node 逐字节 diff。
- `.data-e2e/`、`.data-test/`、`aluka_build/` 为运行期产物，已在 `.gitignore` 中忽略。
