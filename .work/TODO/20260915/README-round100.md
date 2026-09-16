# 2026-09-15 · 续轮 TODO（M7.2 轮一百：真实主流 npm 包加载实测 — lodash / zod / chalk / commander）

> 总 TODO 见 [../README.md](../README.md)；上一轮见 [./README-round99.md](./README-round99.md)。
> 证据规则见 [../README.md](../README.md) §0。

**当前里程碑**：M7（M7.2 真实生态承载 → M7.3 npm Top 50 无缝运行）　|　**权威 Oracle**：Node.js 22 LTS（实测 v22.3.0）

**本轮范围**：改用**真实主流包**驱动（而非继续打磨 Error/原型细节）——新建
`demo/npm-ecosystem-demo`，安装 `lodash` / `zod` / `chalk` / `commander`（真实 npm registry），
逐包做「加载 + 典型 API 调用」对拍，暴露下一批引擎缺口并修复其中可在本轮闭环者。

---

## 1. 待办（开工先登记）

| # | 待办任务项 | 状态 |
|---|---|:---:|
| 1 | 建 `demo/npm-ecosystem-demo` 并经 **npm** 安装 lodash/zod/chalk/commander | `[x]` |
| 2 | 逐包「加载 + 典型 API」探针，与 Node 逐行对拍（先取基线） | `[x]` |
| 3 | 定位并修复阻塞类缺陷（可闭环者） | `[x]` |
| 4 | 覆盖 / 已修项回归复验 | `[x]` |
| 5 | 门禁（fmt / clippy / 全量 test） | `[x]` |
| 6 | 证据回填与 `git diff` 复审 | `[x]` |

---

## 2. 实测证据（已回填）

探针位于 `demo/npm-ecosystem-demo/probes/`（依赖：lodash 4.17.21 / zod 3.23.8 /
chalk 4.1.2 / commander 12.1.0，真实 npm 安装，含 ansi-styles/color-convert 等
传递依赖）。对拍方式：`node probes/X.js` 与 `aluka run probes/X.js` 输出逐字节 diff。

**终验（门禁后复跑）**：10/10 探针与 Node **逐字节一致**——

```
FINAL: PASS=10 FAIL=0 (共10)
  load-probe       加载四包全部 ok（lodash typeof=function / zod keys=109 / chalk level / commander Command）
  lodash-api       35 项典型 API（含 _.template / camelCase 族 / isEqual / chain）
  zod-binding      CJS 互操作链（__createBinding/__exportStar + getOwnPropertyDescriptor 面）
  chalk-api        样式链（red/bold/rgb/hex/bg/嵌套链/模板标签）
  commander-api    子命令/选项/variadic/parse
  static-* ×2      静态属性面
  repro-with       with 语句（读取/回写/遮蔽）
  repro-bindings   上下文关键字形参 + for-of 逐次迭代绑定隔离
  chalkshape       chalk 形态最小复现（对象字面量 getter 捕获循环绑定）
唯一登记差异：repro-r100 的 unicodeSize_astral（1 vs Node 2，见 §5 后续项 1）
```

基线（修复前）关键差异记录：
- 加载探针四包全 FAIL/半 FAIL（new.target、类字符串成员名、Object.keys/create/getOwnPropertySymbols、
  Array.isArray 属性面缺失——均为本轮开工时工作区已有修复，本轮验证收敛）
- lodash：template SyntaxError、camelCase/kebabCase/startCase 词法切分错、pad 长度差一
- zod：109 个导出键枚举顺序全反 + 描述符 configurable 报 true
- chalk：全部样式渲染为 `\u001b[100m`（bgGrey 码）、rgb 分量 NaN

## 3. 缺陷与修复（已回填，8 项）

| # | 缺陷 | 根因 | 修复 |
|---|---|---|---|
| 1 | `_.template` SyntaxError | 解析器不支持 `with` 语句 | 新增 `Stmt::With`（parser）+ `PUSH_WITH_SCOPE=110`/`WITH_RESTORE=111` 双指令 + VM `with_scopes` 动态作用域栈（LoadGlobal/StoreGlobal/TypeofGlobal 优先经对象环境解析；invoke_function 跨帧隔离；break/continue/catch 着陆统一绝对深度截断） |
| 2 | camelCase/kebabCase/startCase 切分错（`[" ","-","z"]`） | 正则 `\xHH`/`\uHHHH`/`\u{...}` 转义未实现，`\x00` 被拆成字面量 x/0/0 | aluka-regex 两条转义路径补全（ClassItem 改码点域 `Range(u32,u32)`+`Cp(u32)`） |
| 3 | `hasUnicode('abc')` 误判 true / pad 走 unicode 路径 | 代理区码点类 `[\ud800-\udfff]` 无法用 `char` 表示 | 匹配器按 UTF-16 代理码元仿真：天文层字符的高/低半区参与码点成员判定 |
| 4 | zod 导出对象枚举顺序全反 | 访问器仅存 `HashMap`（无插入序） | `defineProperty`/对象字面量/类三路注册时把键占位进**有序属性存储**，枚举面回到插入序 |
| 5 | zod 描述符 `configurable:true`（应 false） | defineProperty **访问器分支**漏登记 `non_configurable` | 补登记（数据分支本就正确） |
| 6 | chalk 全样式渲染 `[100m` | **for-of 逐次迭代绑定无隔离**——闭包捕获共享槽位见末次值 | 复用既有 head/iter 双槽 + `CloseUpvalues` 封印机制，扩展到 ForIn/ForOf 头部绑定（含解构）与全部循环体块级绑定；补 `IndexAssign`/`MemberAssign` 捕获检测臂 |
| 7 | color-convert `from is not defined` | `function link(from, to)`——上下文关键字形参不被接受 | 三处形参臂改走 `advance_ident_like()`（context_ident 集合） |
| 8 | `Object.keys` 属性面直调泄漏数组 length/符号键 | 属性面 handler 未做 CALL_METHOD 硬编码分支同款过滤 | keys 排除 length+符号键；getOwnPropertyNames 排除符号键（保留 length，规范如此） |

## 4. 门禁（已回填，真实输出）

```
cargo fmt --all --check                       → 通过（FMT-OK）
cargo clippy --all-targets --all-features
  -- -D warnings                              → 0 warning / 0 error（grep 计数 = 0）
cargo test --workspace --all-features
  --no-fail-fast
  -- --skip tty_surface_e2e_matches_go
  --skip readline_eof_close_e2e_matches_go    → TEST_EXIT=0，92 个套件 test result: ok，0 FAILED
```

回归修复过程证据：首轮全量暴露 2 处本轮引入回归（`Object.keys` 数组 length/符号键泄漏 →
conformance gen-final-matrix-0002 与 symbol_property_keys_match_go；操作码回环测试边界
110→111 段），均已修复并复跑全量至绿。

## 5. 登记后续项（待回填）

1. **正则引擎 UTF-16 码元精确仿真**：astral 串的 exec 计数 1 vs Node 2（`unicodeSize`
   的 `result>1` 回退使 pad 等消费方不受影响；仅影响「混排 astral 串尺寸」边缘）。
2. **with 边界**：with 体内定义的闭包对外层名的动态解析（当前仅同函数体内指令生效）；
   生成器/async 帧内 `with` 的挂起-恢复保持。
3. **迭代隔离覆盖缺口**：`for (let i...)` C 循环 update 写的捕获可见性极边缘（本实现
   update 落 raw 槽）；生成器帧内循环绑定隔离；jit_helpers.rs 的 getter 注册路径未加
   有序占位（JIT 回退解释器，仅枚举次序面）。
4. **Closure 变体访问器枚举序**：函数对象静态面 getter（body-parser 形态）仍 HashMap 序。
5. **own_entries 访问器值面**：`JSON.stringify` 对访问器属性应调用 getter（当前返回
   访问器函数值/占位，既有偏差沿用）。
6. **严格模式 `with` 禁令**：`'use strict'` + with 应 SyntaxError（当前解析接受）。
7. 既有（非本轮引入）：aluvm bin 在无 `runtime` feature 下编译报 E0433（aluka_runtime
   依赖未门控），官方门禁 `--all-features` 不受影响。

## 6. 变更面（git diff 复审）

16 文件 +813/−47：aluka-regex（转义+码点域）、aluka-bytecode（op 110/111 登记）、
aluka-parser（with 语句/关键字表/形参臂）、aluka-compiler（with 编译/循环隔离/捕获检测）、
aluka-vm（with 栈/访问器占位与 configurable/keys 过滤/class+字面量占位/call 帧隔离）。
demo 探针 8 个新增/修订（probes/、不含 node_modules）。
