# vue-sfc 差分 gate——gap 台账

驱动式修复循环的审计记录（`docs/vue-compiler-sfc-dev-plan.md` §4）。
每条 gap：现象 / 最小复现 / 规范依据 / 修复 commit，闭环后标记 ✅。

| # | 状态 | 现象 | 根因 | 修复 |
|---|------|------|------|------|
| G1 | ✅ 已闭环 | 探针死于 `TypeError: Object prototype may only be an Object or null`（postcss `_inheritsLoose` → `Object.create(superClass.prototype)`） | `Object.defineProperty` 部分描述符丢失 `value`（描述符只给 `writable` 时把现值重置为 undefined），且 `writable/enumerable/configurable` 完全不执行。7 行复现：`function N(){}; N.prototype.w=1; Object.defineProperty(N,'prototype',{writable:false}); typeof N.prototype` → node `object` / 旧 aluka `undefined` | `engine.DefineOwnProperty`（规范子集：部分描述符合并/标志执行/校验）+ `objectValue.attrs` 惰性标志字典 + IC `SetCached` 不可写守卫 + `ObjectUnwrapper`（Closure/NativeMethod/vmClosure 解包） |

## 修复 G1 期间发现的次生缺口（均已闭环）

- **函数对象访问器丢失**：`Object.defineProperty(fn, 'g', {get(){...}})` 后 `fn.g === undefined`。
  根因：`Closure` 等包装类型靠委托实现 `engine.Object`，engine 侧类型 switch 认不出，
  静默走退化路径。修复：`engine.ObjectUnwrapper` 可选接口 + `unwrapObjectValue`，
  `DefineOwnProperty/AttrsOf/AllOwnKeys/GetOwnSlot` 统一解包。
- **SameValue 对数值不可靠**：`numberValue` 是包着 `*numberBox` 的值类型（接口存值形态），
  指针断言失败后落到结构体 `==`（比较不同 slab box 指针）→ 等值重定义误判为不等。
  修复：`sameValue` 按数值比较（含 NaN 相等）。

## 当前边界

- `Object.defineProperty`、`Reflect`、数组 exotic properties、Proxy invariants、`preventExtensions/seal/freeze` 和 JIT 属性写入均已接入同一描述符语义并有回归覆盖。
- SameValue 已区分 `+0/-0`，并正确处理 NaN；符号 own keys 在 names/symbols/Reflect.ownKeys API 间保持分类。
- fallback 正则已使用 UTF-16 可见索引；非 `u` 模式按 code unit、`u` 模式按 code point 匹配。孤立 surrogate 在当前 Go UTF-8 字符串物化路径中可能显示为 U+FFFD，但索引和匹配边界保持正确。
- official 后端支持普通 script、script setup、TypeScript、named exports，以及 `<script src>` / `<template src>` / `<style>`（纯 CSS，含 scoped：Go 选择器后缀，与 Vite `data-v-*` 一致）。custom block、预处理器、CSS modules、`:deep`/`:slotted`/`:global`/`v-bind()` 仍明确报错，不会静默丢弃。


## M4（official bundler 后端）实现要点

- `internal/bundler/vue/official.go`：构建 VM 上经 `module.Loader.RequireModule`
  加载 `vue/compiler-sfc`（SetNoCache 保持 webbuild 缓存约束；`builtin.RegisterAll`
  补 path/util 等内置；裸 VM 补 `process.env.NODE_ENV=production` 与 no-op console）。
- 驱动器挂接采用 Vite 同款模式：`export default` → `const __sfc__` +
  `__sfc__.render = render`——遗漏挂接时选项式组件无 render，SSR 渲染为 `<!---->`
  空占位（已由集成测试锁定）。
- require 解析基准传入口**文件**（resolver 从父文件目录向上爬 node_modules，
  传目录会差一层）。

## M3（正则加固）闭环记录

- `tools/extract-regex-corpus.mjs` 从 compiler-sfc 依赖闭包提取 182 个去重模式；
  174 个模式可同时由 RE2 翻译层与自研回溯引擎编译，×30 个领域/通用输入
  共 5220 次逐捕获组索引对拍零差异（8 个模式因任一侧不支持明确 skip）。
- 对拍驱动修复三类真实语义 bug（Node 22 为裁判）：
  1. 懒量词位于捕获组内时，回退多吃字符后未更新捕获终点（`(.*?)$`）；
  2. 字符类转义未区分单 rune 与集合转义，作为区间端点时以 `lo=0`
     误构造超宽区间（`[ -,\.\/:-@\[-\^`{-~]`）；
  3. `[^]` 语义二次取反：旧实现给 negated 空类塞全范围 part 后再整体取反，
     变成永不匹配；现以 negated + 空 parts 自然表示任意字符。
- 回溯引擎增加一次 exec 全局共享的 `btMaxSteps=2^22` 步数预算，覆盖候选
  起始位置和 lookaround 子状态；超限保守返回不匹配。测试经低预算
  `execWithLimit(...,32)` 明确断言 `aborted=true`，并验证正常匹配不被误杀。
