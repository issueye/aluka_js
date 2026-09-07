# 2026-09-08 · 每日 TODO（M2.4 启动轮：全局函数面 + 真实包加载修复）

> 总 TODO 见 [../README.md](../README.md)；证据规则见其 §0。

**当前里程碑**：M2.4 Express 真实依赖树（推进中）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 本轮目标

1. M2.4 启动：安装 express 真实依赖树（npm install）、alukac build 全树预编译、逐包排障；
2. 以排障发现的共性缺口反哺引擎（全局函数面 / 字符串相等 / GC 根完整性 / 嵌套 require 栈隔离）。

---

## 2. 完成状态

| # | 任务项 | 状态 |
|---|---|:---:|
| 1 | fixture：`demo/express-demo`（npm install express@4 → 68 包）+ 6 场景自测 app（GET / 、GET /echo/:word、POST /json、并发、自定义 Content-Type、优雅退出），node oracle 输出已固化 | `[x]` |
| 2 | alukac build 全树预编译：**131 模块编译成功、0 失败**（修复解析器把上下文关键字 `from` 当保留字——`var from = ...`，mime-types） | `[x]` |
| 3 | 排障修复①：**ECMAScript 全局函数面缺失**——新增 builtins/global_fns.rs：isNaN/isFinite/parseFloat/parseInt、Number（转换+静态面 MAX_SAFE_INTEGER 等）、Boolean、encodeURIComponent/decodeURIComponent/encodeURI/decodeURI、Date（now/parse/实例 getTime·toISOString）、globalThis（属性直通全局表） | `[x]` |
| 4 | 排障修复②：**数组 indexOf 族按句柄比较字符串**——新增 string_values_eq 内容相等并接入 indexOf/lastIndexOf/includes（Array 与 TypedArray） | `[x]` |
| 5 | 排障修复③：**嵌套 require 操作数栈泄漏**——call_require 对嵌套模块体执行做栈基线隔离（truncate），修复 `module.exports = require(...)` 在真实包中的栈序污染 | `[x]` |
| 6 | 排障修复④：**GC 根不完整**——require 进行中的 module 对象加 `gc_pinned` 钉扎（嵌套加载期间占位/module 对象被回收导致后续读取 undefined） | `[x]` |
| 7 | 门禁全绿 + 证据回填 | `[x]` |

## 3. M2.4 当前阻塞点（诚实登记，下轮继续）

`require('send')` 在模块顶层抛 `Cannot read properties of undefined (reading '1')`
（疑似 `process.version`/版本号解析类引擎缺口），阻塞 qs 与 express 本体的加载验证；
`accepts` 导出为 string 而非函数（模块初始化路径仍有语义缺口）。

**下轮排障入口**：send/lib/send.js 顶层语句逐行定位 `x[1]` 来源 → 修复 → 顺推 qs、
express 本体、express.json()（body-parser 的流式 body 读取）→ 6 场景对拍。

---

## 4. 自动化门禁结果（全绿）

```bash
cargo fmt --all --check          # FMT-OK
cargo clippy --all-targets --all-features -- -D warnings   # 0 error
cargo test --workspace --all-features
# passed: 536, failed: 0（含 test262 154 例、node22 conformance 18 例含 18-tla-dag）
```

---

## 5. 复审结论

- 本轮全部改动由真实包排障驱动（mime-types `var from` 解析、ms 的 isNaN、
  debug 的链式赋值依赖的全局面、accepts 的内容相等），无投机性改动；
- fixture（demo/express-demo）含 node_modules 安装产物，app.js 为 6 场景自测脚本，
  node oracle 输出见 git 历史（`PORT_READY / GET / -> / ECHO / POST / CONCURRENT /
  CTYPE / CLOSED`）。

---

## 附：M2.4 深度排障记录（第二轮，2026-09-08 深夜）

### 定位历程
1. `require('debug')` 返回空对象 → 全局面缺失（上轮修复的衍生确认）；
2. `ms(5)` 抛 "undefined is not a function" → **isNaN/parseFloat 等全局函数缺失**（已修）；
3. `mime-types` 的 `var from = ...` 解析失败 → **上下文关键字 from 被当保留字**（已修）；
4. `accepts`/`mime-types` 的 `indexOf('iana')` 返回 -1 → **数组 indexOf 按堆句柄比较字符串**（已修，改内容相等）；
5. `statuses` codes.json 加载后 http-errors 仍失败 → **嵌套 require 操作数栈泄漏**（已修，栈基线隔离）；
6. `module.exports` 重赋值后缓存指向占位对象 → 收尾重读机制确认。

### 遗留精确缺口（下轮恢复入口）
`depd` 的 `eehaslisteners`（func_idx=25，num_params=2）被以 **args=[]** 调用：
- 现象：`TypeError: Cannot read properties of undefined (reading 'listenerCount')`，last_pc=1（eehaslisteners 的 GET_PROP listenerCount）；
- caller：func=-1（inline main，run_func 不设置 current_func_idx，无法区分具体模块）；
- 已排除：resolve_global("process") 正常返回对象；LoadGlobal process 入栈正确；statuses/inherits/setprototypeof/toidentifier 均单独加载成功；
- 怀疑方向：MakeClosure 操作数改写与调用点实参压栈的交互（http-errors main 的某条零参 CALL 落到了 eehaslisteners 上）；
- 建议工具：给 VM 增加轻量调用栈（frame 链表），一次性解决此类「caller 是谁」的诊断难题。
