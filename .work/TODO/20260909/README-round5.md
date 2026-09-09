# 2026-09-09 · 每日 TODO（M5 续轮 round5：M5.3 node:sqlite Node 22 对拍补齐）

> 总 TODO 见 [../README.md](../README.md)；M5 评审登记见 [./README.md](./README.md) §15。
> 证据规则见 [../README.md](../README.md) §0；门禁命令见 AGENTS.md §3。

**当前里程碑**：M5 多线程并发与系统级扩展（续轮）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

## 1. 本轮目标（可判定完成态）

承接 §15 评审建议顺序第一项——**M5.3 对拍补齐（成本低、验收价值高）**：

1. 编写覆盖 sqlite 主路径的差分探针（CRUD/语句结果/事务包装器/回滚/bigint/
   blob/错误形态/columns/sourceSQL/close 语义等），Node 22 实跑取 oracle；
2. aluka 同脚本输出与 Node 逐字 diff，逐项修复差异（错误文本、类型映射、
   `columns().type`、事务路径 `isTransaction` 等评审缺口）；
3. 在 `builtins_phase7_io_test.rs` 中新增 `assert_e2e_matches_node` 真对拍
   用例（既有 4 个 e2e 的 `assert_e2e_matches_go` 别名不比对 Node——评审
   指出的验收证据缺口），或新建独立 sqlite 对拍测试文件；
4. 门禁全绿 + 证据回填 + 登记 M5.3 判定（达标后总 README 改 `[x]` 或按
   实际缺口续登记）。

## 2. 待办清单

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | sqlite 差分探针设计 + Node 22 oracle 实跑 | `[x]` | 3 轮语义采集 + 44 行 oracle(node-out.txt) |
| 2 | aluka vs Node 逐字 diff + 差异修复 | `[x]` | 首轮 3 差异 → 全消;44 行 IDENTICAL |
| 3 | `assert_e2e_matches_node` 真对拍用例固化 | `[x]` | `probes/node22_sqlite_probe.js` + `sqlite_node22_diff_e2e_matches_node` 1/1 |
| 4 | 门禁三连 + 登记 | `[x]` | 见下 |

## 2b. 交付摘要（M5.3 判定：**达成可验收**，遗留口径登记）

### Node 22.23.1 实测语义修订（评审登记多处假设被实测推翻）

1. **`db.transaction(fn)` 不存在于 Node 22 LTS**（原型面实测
   `open,close,prepare,exec,function,location,aggregate,…`；Node 23.8+ 才有）
   ——Aluka 保留为**超集扩展**（模块头注释更新），事务对拍以
   `exec(BEGIN/COMMIT/ROLLBACK)` + `isTransaction` 为 oracle；
2. **错误文本全面 Node 化**（原实现复刻 Go modernc 形态，逐字不符）：
   `message` = errmsg 原文（无 `node:sqlite:` 前缀/`(extcode)` 尾缀）；
   SQL 错误挂 `code: ERR_SQLITE_ERROR` + `errcode`（扩展码，INTEGER PRIMARY
   KEY 冲突实测 **1555**=SQLITE_CONSTRAINT_PRIMARYKEY）+ `errstr`（主码文本）；
   参数 TypeError 挂 `code: ERR_INVALID_ARG_TYPE`；CANTOPEN 消息裁路径尾缀；
3. **绑定规则**：缺位置参数 → 未绑定占位按 NULL 补齐（不报错）；**超位** →
   `column index out of range`；未知命名键 → `Unknown named parameter 'k'`；
   单对象参数（**含数组**，数字键展开）→ named；`undefined`/布尔/普通对象 →
   TypeError（带参数序号）；命名绑定失败序号 = 语句占位符位置；
4. **columns()**：Node 五键 `column/database/name/table/type`（expression 列
   置 null、type=decltype）——经 rusqlite 安全 API `columns()` +
   `columns_with_metadata()`（origin/table/database 与 Node 同源，alias 亦
   取源列名；新开 feature `column_decltype`+`column_metadata`），**零 unsafe**；
5. **iterate 结束后 `next().value` = null**；close 语义：二次 close /
   close 后 prepare-exec → `database is not open`，close 前 prepare 的 stmt
   → `statement has been finalized`；`run` 对 SELECT 不报错（changes/rowid
   回读最近写值，rusqlite `ExecuteReturnedResults` 特判）；
   path/sql 参数 validator（TypeError，Node 文本）。

### 验证证据

- 探针 44 行输出与 Node 22.23.1 **逐字一致**（CRUD/结果形态/参数/named 边界/
  columns JSON/事务/isTransaction/bigint/blob/错误 attrs/close/validator）；
- 新增 Rust e2e `sqlite_node22_diff_e2e_matches_node`（真对拍）+ 既有 4 用例
  断言文本更新为 Node 形态——`builtins_phase7_io_test` 13/13；
- 门禁：fmt 0 ｜ clippy 全零 ｜ **workspace 全量 561 passed / 0 failed**。

### 遗留登记（不阻塞 M5.3 验收）

- 预编译语义：语句每次执行重编译（语义等价，非句柄预编译）；ctor options
  （open 标志/readOnly 等）未实现；裸名 `require('sqlite')` 可用（Node 应
  MODULE_NOT_FOUND，剥前缀折衷既有登记）；`isTransaction` 在超集
  `transaction()` 包装器路径不同步（Node 22 无此 API，wrapper 为 Aluka 扩展）。

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                        # FMT-OK（exit=0）
cargo clippy --workspace --all-targets --all-features -- -D warnings  # 0 error 0 warning
cargo test --workspace --all-features          # 561 passed / 0 failed（含 sqlite_node22_diff 真对拍）
```

## 4. 提交

```bash
git commit -m "feat(m5.3): node:sqlite 对齐 Node 22.23.1 实测——真对拍闭环 + 引擎 Dict O(n²) 修复 + fetch redirected + e2e 超时防护"
```

## 5. 遗留/下轮

- M5.2 P0（bc 模式 cluster+fetch ≥2 挂死）→ M5.1 结构化克隆 → M5.4 Timer Mock；
- 引擎属性面缺口（新登记）：`new Uint8Array([..]).constructor`/`Array.from`/
  `obj.constructor` 缺失、TypedArray 原型未挂（`[object Object]` 非
  `[object Uint8Array]`）——sqlite 对拍以 `Buffer.isBuffer`+字节面绕行，
  真实程序常用 `Buffer.from(blob)` 已可用（extract_bytes 现支持
  TypedArray/ArrayBuffer/DataView）。
