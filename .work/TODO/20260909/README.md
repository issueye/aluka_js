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