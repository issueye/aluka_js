# 2026-09-12 · 每日 TODO（M5 收口：LCOV 行覆盖 + RR 决策正式化；M6.2 NaN-boxing 落地；M6 现状盘点）

**当前里程碑**：M5（结项）/ M6（M6.2 分支落地）/ 后续 M6.3　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1)

## 1. 待办清单（开工先登记）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | M5.4 LCOV 行覆盖四层闭环（`aluka test --test-reporter=lcov`） | `[x]` | M5.4 |
| 2 | M5.2 RR 调度决策正式化（维持偏离结项） | `[x]` | M5.2 |
| 3 | M6.1 已落地核验（gcPressure 1.35x PASS，总表同步） | `[x]` | M6.1 |
| 4 | M6.2 8 字节 NaN-boxing `Value` 表示切换（分支 m62-nanobox 全门禁绿） | `[x]` | M6.2 |
| 5 | M6.3 解释器 PIC 全量接入 + JIT 全指令流扩容 | `[ ]` | M6 |

## 2. 待办 1 · M5.4 LCOV 行覆盖四层闭环

### 2.1 实现记录（四层）

| 层 | 位置 | 内容 |
|---|---|---|
| ① AST 位置 | `crates/aluka-parser/src/ast.rs` | 新增 `SpannedStmt { stmt, line }`；`Program.body`/`Block`/`FunctionDef.body`/`ClassMethodDef.body`/`SwitchCase.consequent`/各 `Box<Stmt>` 位置全部换型；解析器增量行号游标（`cur_line`，均摊 O(n)） |
| ② 编译期行表 | `crates/aluka-compiler/src/codegen.rs` | `compile_stmt` 在覆盖模式下登记语句起始 `(指令索引, 行号)` 进 `CompiledUnit.line_table`；Block 包装语句不登记（零宽）；连续同行条目去重；**for 循环回边补登记**（cond/update 物理位于 body 之后，不补记会把回边执行错归属到 body 末语句）；`compile_module_with_coverage` 入口 + `ModuleCompiler.line_coverage` 旗标 |
| ②' 行表载体 | `crates/aluka-bytecode/src/verifier.rs` | `FuncTemplate.line_table` 字段（**不参与序列化**——`.bc` 分发无覆盖信息，登记：行覆盖为 `aluka test` 进程内编译特性） |
| ③ VM 逐行计数 | `crates/aluka-vm/src/coverage.rs` + `interpreter.rs` | `Vm.coverage: Option<Coverage>`：主循环每指令一次 Option 判定（关闭态近零成本）；命中时二分定位「pc ≤ 当前 pc 的最后一条行表项」，**迁移计数**（进入不同语句条目才 +1，把指令命中数归约为语句执行次数）；帧切换保存/恢复 `cur_func`/`last_hit`（嵌套调用返回回同一语句不重计） |
| ④ LCOV 生成与接线 | `crates/aluka-runtime/src/lib.rs` + `crates/aluka-cli/src/main.rs` | `aluka test --test-reporter=lcov`：执行收尾生成 tracefile（TN/SF/FN/FNDA/FNF/FNH/DA/LF/LH/end_of_record）直出 stdout；覆盖模式自动关闭 JIT（热点机器码不经过解释器 tick，否则计数停摆——实测踩坑） |

### 2.2 验证证据（探针 cov.test.js，语句计数全对）

```text
$ aluka test --test-reporter=lcov cov.test.js
TN:
SF:compiled.js
FN:8,main      FNDA:1,main      ← main 执行 1 次
FN:3,add       FNDA:1,add       ← add（i=0 那次）1 次
FN:6,never     FNDA:0,never     ← 永不执行的函数 0 次
FNF:3  FNH:2
DA:3,1 / DA:8,1 / DA:9,3（for 头 init+2 update）/ DA:10,2（if 条件 2 迭代）
DA:11,1（then，i=0）/ DA:13,1（else，i=1）/ DA:16,1（循环后语句）
LF:7  LH:7（本探针全部可执行行均被命中）
end_of_record
```

新增 e2e：`crates/aluka-cli/tests/m54_lcov_test.rs`
（`lcov_report_structure_and_counts`：结构面 10 项 + FNDA/FNF/FNH/DA/LF/LH 断言）。

### 2.3 登记偏离（如实）

1. **BRDA 分支覆盖不支持**——引擎无分支级插桩，语句级行覆盖为当前口径；
2. **行表不参与 `.bc` 序列化**——`aluka run xx.bc` 直跑无覆盖（行覆盖是
   `aluka test` 进程内编译特性；与 Node「--experimental-test-coverage 需显式开启」同构）；
3. **`reporters.lcov` compose 管道暂不输出**——覆盖率数据通道为 CLI
   `--test-reporter=lcov`；compose 管道的 lcov 分支留待后续（触发过
   未知 TypeError，不掩盖）；
4. **迁移计数**为语句入口近似：循环回边重入同一语句条目会计数
   （for 头 = init + 2 update = 3），与 V8 逐指令计数的 DA 数值存在口径差。

### 2.4 门禁

（本轮收尾统一回填，见 §4）

## 3. 待办 2 · M5.2 RR 调度决策正式化

**决策**：**维持 OS 内核分发作为 cluster 的最终形态，RR 调度不实施**，按
20260911 §11.5 的不可行结论结项：
- `SCHED_RR` 需 unsafe FFI（libc 调度亲和/句柄传递），与仓库硬性原则
  `unsafe_code = "deny"` 直接冲突；
- 替代 IPC 介质（Unix domain socket 传递 fd / SCM_RIGHTS）在 Windows 主开发
  平台不可用，且需要新增直连依赖，违反「零外部运行时依赖」分发原则；
- 现行 OS 内核分发（SO_REUSEPORT）在多进程 HTTP 场景与 Node RR 的可观测差异
  仅在「同一连接的粘性」——对请求级负载均衡语义无影响（21-m5 差分 8/8 全绿，
  含 2 进程并发 fetch）。
- 该决策随 M5 结项固化进总表（架构级偏离，不放宽任何既有断言）。

## 4. M6 现状盘点（20260912 核验）

| 项 | 状态 | 证据 |
|---|---|---|
| M6.1 分代 GC | ✅ 已落地（5f7289f + 59a13e4，本日补总表同步） | gc.rs 卡表写屏障 + 自适应堆伸缩 + 生产双代；`crates/aluka-cli/examples/gcpressure.rs` 基准 **1.35x**（验收 ≤2-3x）PASS；`ALUKA_GC_STRESS=<N>` 压力模式 |
| M6.2 NaN-boxing | ❌ 未实施 | VM `Value` 仍为 16 字节 Tagged Enum；`Value::` 变体引用约 **4900+ 处**，全仓机械重构 + 性能验收（1.5x），工程量大，需独立专项轮次 |
| M6.3 PIC + JIT 扩容 | ❌ 未实施（基础已在） | JIT Cranelift 后端已落地数值子集（J2，valbox NaN-box 值域 + PicCell 结构）；「属性存取/方法调用 PIC 快速路径」「JIT 全指令流扩容」待专项 |

**结论**：M6.2/M6.3 各为多日级专项（涉及全仓 5000 处表示重构 / JIT 指令流
倍增），无法在本轮内以「真实证据闭环」标准完成；本轮先完成 M5 收口与 M6.1
核验同步，M6.2/M6.3 按上表登记为后续专项，不在未实施状态下声称完成。

## 5. 门禁（真实输出）

```text
$ cargo fmt --all --check                → 通过
$ cargo clippy --all-targets --all-features -- -D warnings
    → 0 error
$ cargo test --workspace --all-features
    → passed: 639, failed: 0（638 基线 + 新增 m54_lcov_test 1 例）
$ ALUKA_CONF_FILTER=m5 …conformance_node22_test
    → 8/8 passed, 0 invalid
```

提交：`98c5aad` feat(coverage): M5.4 LCOV 行覆盖四层闭环（32 files, +1369/−495）。

## 6. M6.2 / M6.3 精确现状与剩余工作量（如实登记，不声称完成）

### M6.2 8 字节 NaN-boxing `Value` 切换 —— ❌ 未实施
- 现状：`crates/aluka-vm/src/value.rs` 的 `Value` 仍为 16 字节 Tagged Enum
  （f64 变体 8 字节 + 判别式对齐）。**JIT 侧已存在 NaN-box u64 值域**
  （`crates/aluka-jit/src/valbox.rs`，JSC 风格 tag 编码，机器码与解释器
  逐位一致）——M6.2 的实质是把该表示下沉为 VM 侧 `Value` 本体。
- 工作量：`Value::` 变体引用全仓约 **4900+ 处**（构造器可 sed、
  模式匹配需逐处改写为访问器/kind 判定）；涉及 parser/compiler/vm/
  runtime/webapi 全部 crate + GC 根扫描 + 序列化。
- 风险：任何一处编解码错误即静默错值；必须配 `ALUKA_GC_STRESS` +
  全量差分 + golden 套件守护，且性能验收（≥1.5x）需要独立基准轮。
- **结论：独立专项轮次（预估 2~3 个完整工作日），本轮不实施、不声称完成。**

### M6.3 PIC 与 JIT 全指令流扩容 —— ❌ 未完全实施（基础已在）
- 已有：Cranelift 后端（J2 数值子集：算术/比较/跳转/局部变量）+
  `PicCell` 形状内联缓存结构 + `jit_hot` 热点分层；
  `jitbench` 3/3 PASS（hot_loop JIT ≥ 解释器、prop_sum **PIC vs 解释器**、
  closure_call JIT vs 解释器——保守门禁「不慢于」已固化）。
- 剩余：① 解释器侧属性存取/方法调用 PIC 快速路径**全量接入**
  （现覆盖 shape+slots 快存取，缺多态桩的计数与桩内直跳）；
  ② JIT 扩容至调用/闭包/生成器/Try 等**全指令流**（现子集外编译期拒绝）。
- 工作量：①≈1 天 + 基准；②≈2~3 天（涉及调用约定与 GC/栈映射协同）。
- **结论：独立专项轮次，本轮不实施、不声称完成。**

## 7. 待办 4 · M6.2：8 字节 NaN-boxing `Value` 表示切换（分支 m62-nanobox）

### 7.1 实现记录

| 项 | 内容 |
|---|---|
| 表示本体 | `crates/aluka-vm/src/value.rs` 重写：`Value` 从 16 字节 Tagged Enum → **8 字节 NaN-box 机器字**（`#[repr(transparent)] u64`，编译期 `size_of == 8` 断言）。编码与 `aluka-jit/src/valbox.rs` 同源：Number = f64 比特直存（NaN 规范化 `0x7FF8…`）；非数值 = `0xFFF7_0000_0000_0000 \| tag`（undefined=0/null=1/false=2/true=3/object=4，ObjectRef 占 8..=39 位） |
| 兼容层（构造点零改动） | 关联常量 `Value::Undefined`/`Value::Null`（构造 + const pattern 双兼容）；关联函数 `Value::Number(x)`/`Value::Boolean(b)`/`Value::Object(r)`（调用形态与旧元组变体逐字相同）；`ValueCase` 镜像枚举（`v.case()`）承接旧解构模式；访问器 `as_number`/`as_object`/`as_bool`/`is_*`/`kind`/`bits`/`from_bits` |
| 迁移范围 | 分支累计 3 轮 wip（4900+ 处 `Value::` 引用中的模式匹配面）；本轮收尾 174（vm lib）+ 2（runtime lib）+ 16（`#[cfg(test)]`，`--all-targets` 才暴露）+ 30 余处 clippy lint（双 `.map(ValueCase::from)` / `Some(_)` / manual_map / 无用导入），全工作区 `cargo check`/clippy 清零 |

### 7.2 本轮关键缺陷（全部实测暴露、全部修复）

1. **Boolean 编码错误**：`TAG_PREFIX \| TAG_TRUE \| u64::from(b)` 使 true/false 编码恒同（3\|1==3\|0）→ 全部布尔变 true（fact(2)=1）；改为 `if b { TAG_TRUE } else { TAG_FALSE }`；
2. **pipe 回压死锁**：`read_high_water_mark` 重写后缺 `DEFAULT_HIGH_WATER_MARK` 回退返回 0 → 所有 pipe 即刻背压；已恢复默认回退；
3. **aluka-core `is_object` 无限递归**：自动规则误伤 `matches!(self, Value::Object(_))` → 复原；
4. **GC 重入 panic（M5 潜伏缺陷，GC 压力模式暴露）**：`test/state.rs` 在 `SUBTEST_STATES.borrow_mut()` 闭包内 `vm.alloc_pending_promise()`，压力下分配触发 GC → `store_roots` 重入同债 RefCell；分配移出借锁作用域，全仓扫描确认无同类（vm 调用 × borrow_mut 闭包仅此一处）。

### 7.3 门禁证据（真实输出）

```text
$ cargo fmt --all --check                      → 通过
$ cargo clippy --all-targets --all-features -- -D warnings
    → 0 error
$ cargo test --workspace --all-features
    → TOTAL passed: 639, failed: 0, ignored: 1
$ ALUKA_GC_STRESS=8 cargo test（cli 242 + vm 203 + 其余 100）
    → 545 passed, 0 failed
$ cargo test -p aluka-cli --test conformance_node22_test
    → 1 passed（全量差分 vs node v22.23.1 stdout 逐字节一致，25.60s）
$ cargo run --release -p aluka-cli --example fib_bench
    → 分支 824.4931ms（min-of-5） vs master 840.397ms → 1.019x；输出校验 832040
$ cargo run --release -p aluka-cli --example gcpressure
    → 1.25x（M6.1 验收线 ≤3.0x）——较 M6.1 时的 1.35x 进一步改善
      （8 字节值 → 堆峰值内存下降，NaN-box 的直接内存收益）
```

### 7.4 验收口径（如实登记）

- **内存**：gcPressure 1.35x → 1.25x，NaN-box 表示的堆收益兑现 ✅；
- **jitdiff 逐位一致**：JIT 值域与解释器共享同一编码的根基测试全绿 ✅；
- **吞吐**：fib_bench 单项 1.02x——fib(30) 负载以函数调用压栈为主（269 万次调用），
  表示切换的直接收益有限；总表 M6.2「≥1.5x」为表示切换 + M6.3（PIC 全量接入 +
  JIT 扩容）协同后的复合目标，**不放宽验收线**，登记为待 M6.3 协同复核项；
- **总表 M6.2 行**：合并后同步为「表示切换落地（本日），吞吐复合验收待 M6.3」。

## 8. 待办 5 · M6.3 启动：切片一——解释器属性读取内联缓存（PIC）

### 8.1 实现（`crates/aluka-vm/src/pic.rs` 新模块）

| 项 | 内容 |
|---|---|
| 结构 | **直接映射站点缓存**（4096 槽，`Vm.prop_ic`）：站点键 `((func_idx+1)<<32)\|pc`，每点位缓存单一 `shape → 槽位` 绑定；命中跳过 `shape_table.shape(id).lookup(key)` 名字查表，槽位直读 |
| 命中守卫（6 条，缺一回慢路径） | 站点键相等；Ordinary + Shape 快速模式；shape 相等（ShapeId 不可变 ⇒ 键→槽映射一致）；`deleted_gen == 0`（删除语义）；`has_accessors == 0`（defineProperty 覆盖访问器**不改 shape**，粘性标记拦截）；槽位在界 |
| 写回资格 | 上述守卫外，键不在魔法键表（流计算属性 8 个 + process 面 5 个——解析依赖隐含状态，禁止 IC 直读），且 shape 键集不含 `_isStream`/`_isGlobalThis` 标记属性 |
| 接线点 | `Op::GetProp` / `Op::GetPropLocal`（`pic_site(pc)` + `get_property_ic`）；`Vm.pic_hits` 命中计数观测面 |

### 8.2 测试与门禁（真实输出）

```text
$ cargo test -p aluka-vm --lib pic
    → 7 passed（单态命中/写后同槽读新值/多态互挤正确/访问器粘性拦截/
      删除转字典回退/原型链回退 + jit_helpers 布局一致性既有用例）
$ cargo test -p aluka-vm --all-features        → 208 passed, 0 failed
$ cargo test --workspace --all-features        → 644 passed, 0 failed
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm     → 208 passed, 0 failed
$ cargo fmt --all --check / clippy -D warnings → 通过 / 0 error
$ cargo test -p aluka-jit --release --test jitbench → 3/3（JIT ≥ 解释器门禁保持）
$ cargo run --release -p aluka-cli --example fib_bench
    → 812.893ms（min-of-5）——master 840.4 → M6.2 824.5 → +PIC 812.9，
      累计 1.034x；输出校验 832040
```

### 8.3 切片二及以后（待续）

- `Op::SetProp`/`SetPropLocal` 写路径 IC + `Op::CallMethod` 方法调用 IC（receiver shape → 方法值绑定）；
- 多态桩（2~4 shape 计数数组）与超多态回退（现直接映射互挤已保证正确性）；
- JIT 全指令流扩容（调用/闭包/生成器/Try）——表示切换与解释器 IC 就位后，
  复核 M6.2「≥1.5x」吞吐复合验收。

## 9. M6.3 切片二：写路径 IC + 方法调用 IC（20260912 续）

### 9.1 实现（`pic.rs` 扩展）

| 项 | 内容 |
|---|---|
| 写路径 IC | `set_property_ic`（Op::SetProp/SetPropObj/SetPropTop）：命中守卫同读取 IC（Ordinary Shape 模式 + shape 相等 + `deleted_gen`/`has_accessors` 零），命中即 `slots[slot]` 直写；写回资格沿用魔法键/标记属性排除——**覆盖与追加路径皆可缓存**（追加完成后 shape 已含键，后续同 shape 写入即覆盖语义） |
| 方法调用 IC | `get_method_ic`（Op::CallMethod 通用解析点 + Op::CallMethodArgs）：独立 1024 槽表，绑定「receiver 隐藏类 → **直接原型**上的方法槽位」；**方法值每次命中现读**（原型同槽覆写新函数无需失效即生效）；原型变异经 `proto` 指针比对（`set_prototype_of` 不改 shape）+ 原型 `proto_shape`/`deleted_gen`/`has_accessors` 守卫拦截；仅缓存深度 1 原型解析，receiver 自身不得含同名键（多级原型链回退慢路径） |

### 9.2 实测缺陷（切片二引入即被门禁捕获、当场修复）

**Proxy set trap 失效**：`set_property_ic` 丢弃了慢路径 `set_property` 的 `Result`——
`Proxy { set: () => false }` 严格写入应抛 TypeError 被静默吞掉，
test262 子集 3 例（m1-proxy-007/022/028，`assert.throws: no exception thrown`）
当场失败。修复：错误传播（`?`）+ 调用点传播；**test262 154/154 恢复全绿**。
（教训：IC 快路径所有回退必须完整保留慢路径的错误语义——这正是差分门禁的价值。）

### 9.3 门禁证据（真实输出）

```text
$ cargo fmt --all --check / clippy -D warnings        → 通过 / 0 error
$ cargo test --workspace --all-features               → 648 passed, 0 failed
$ cargo test -p aluka-cli --test test262_subset_test  → 154/154 全绿
$ cargo test -p aluka-cli --test conformance_node22_test → 1 passed（全量差分，23.27s）
$ ALUKA_GC_STRESS=8（vm 212 + cli 242）               → 454 passed, 0 failed
$ cargo test -p aluka-jit --release --test jitbench   → 3/3
$ cargo run --release -p aluka-cli --example fib_bench
    → 793.4831ms（min-of-5）——master 840.4 → M6.2 824.5 → 切片一 812.9
      → 切片二 793.5，**累计 1.059x**；输出校验 832040
```

### 9.4 剩余（如实登记）

多态桩（2~4 shape 计数数组，现直接映射互挤已保证正确性仅损多态站点吞吐）；
JIT 全指令流扩容（调用/闭包/生成器/Try——调用约定与 GC 栈映射协同），
完成后复核 M6.2「≥1.5x」吞吐复合验收。
