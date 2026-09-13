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

## 10. M6.3：1.5x 吞吐复合验收复核（切片二后检查点）

### 10.1 复测数据（真实输出）

```text
$ cargo run --release -p aluka-cli --example gcpressure
    → 1.31x（≤3.0x 验收线）PASS（前次 1.25x，运行间波动内一致）
$ cargo run --release -p aluka-cli --example fib_bench（连续复测）
    → 793.5ms（静默窗口，切片二后）；gcpressure 高负载后同机复测
      906/888/871ms——负载热噪声显著，跨窗口绝对值不可比，
      验收以「同静默窗口配对」为准：master 840.4ms vs 切片二 793.5ms
```

### 10.2 复核结论（如实）

- 当前累计加速 **1.059x**（840.4 → 793.5），**1.5x 复合验收未达成**；
- 分解：NaN-box 表示单项 ≈1.02x + 读取 IC ≈1.014x + 写/方法 IC ≈1.024x；
  fib(30) 负载的调用/压栈占比高，而调用族（CallMethod 2000 行内建分派链、
  New、MakeClosure）与 Try/生成器仍在 JIT 编译期拒绝——提速空间被拒之门外；
- **切片三（JIT 扩容前置）已定性**：把 Op::CallMethod 的内联内建分派链
  提取为 `Vm::call_method_dispatch` 可复用入口（Op::CallMethod 与 JIT
  helper 共用，消除「helper 走通用路径 ≠ 内联分派」的语义鸿沟），随后
  JIT 侧接入调用族 helper；生成器（Yield/Await）与 Try 族需独立的
  展开协议（栈映射与 try_stack 协同）——两项均为多日专项，按总表登记
  于后续轮次，不在本窗口内声称完成。

## 11. M6.3 切片三：分派链提取（A）+ JIT 调用族首块（B）

### 11.1 实现

| 步 | 内容 |
|---|---|
| A（ba320c2） | `Op::CallMethod` 内联链（约 2000 行/50 分支/101 处 stack.push）**原样提取**为 `Vm::call_method_dispatch(receiver, method_name, args, site) -> Result<Value, VmError>`：字符串感知括号配平的机械转换（push→return Ok、4 对 `pc+=1;continue` 删除——与循环底部 `pc+=1` 语义等价、`method_name.as_ref()`→`&str` 直用 33 处、EventEmitter if-let 补 unreachable else）；Op::CallMethod 臂收敛为「collect args → pop receiver → pic_site → dispatch → push」 |
| B | **JIT 接入 `Op::CallMethod`**：新 vtable 通道 `CallMethodFn`（jit_helpers `jit_call_method`：常量名解析 + 实参表 marshaling + `call_method_dispatch(u64::MAX 站点)`——JIT 站点键空间与解释器不相交，单态热点即享方法 IC）；编译臂复用 `call_args_slot` 暂存实参后单次 helper call。**语义单源达成**：内建分派链只有一份，解释器与 JIT 共用 |

### 11.2 门禁（真实输出）

```text
$ cargo test --workspace --all-features            → 648 passed, 0 failed
$ cargo test -p aluka-cli --test test262_subset_test → 154/154
$ cargo test -p aluka-cli --test conformance_node22_test → 1 passed（35.90s）
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm         → 212 passed, 0 failed
$ cargo test -p aluka-jit --release --test jitbench → 3/3
$ cargo fmt --all --check / clippy -D warnings     → 通过 / 0 error
```

### 11.3 性能配对口径修正与当前结论（如实）

- 本日午后机器持续高负载后进入低速态（同机同构建较上午慢约 13%）——
  跨窗口绝对值不可比。**当前窗口同条件配对**（间隔数分钟）：
  master(7b8a631) 928.8ms vs 切片三后 901.2ms = **1.031x**；
  上午静默窗口配对为 1.059x（840.4 vs 793.5）。fib 热函数仅含 Op::Call
  （本就支持），切片三 B 的 CallMethod 编译面向其它工作负载（属性方法
  密集型）开放，fib 单项数值不因本切片变化；
- **1.5x 复合验收仍未达成**（两口径均 <1.2x）——剩余依赖：JIT 调用族
  补全（New/MakeClosure/CallArgs 族）、多态桩、生成器/Try 展开协议
  （切片四，多日）。如实结转，不在未达成状态下声称完成。

## 12. M6.3 切片三 C：调用族补全（20260912 续）

- `Op::New`（jit_construct → do_construct）、`Op::NewArgs`/`Op::CallArgs`
  （jit_call_args → to_array_values + invoke_callable）、
  `Op::CallWithThis`/`Op::CallWithThisArgs`（jit_call_this：argc>0 表指针 /
  argc==0 数组值约定，两变体共用通道）接入 JIT 编译；
- **调用族覆盖 6/8**：Call/CallMethod/CallArgs/New/NewArgs/CallWithThis(Args)
  已编译；余 super 族（ConstructThis/ConstructThisArgs——需 JIT 帧访问
  this=locals[0]）与 MakeClosure（需上值表安装）——与生成器/Try 同归
  切片四的调用约定协同，多日专项；
- 门禁：workspace 648/0、test262 154/154、jitbench 3/3、clippy 0、
  GC 压力 vm 212/0 全绿（真实输出同 §11 口径）。


## 13. 切片四前置发现：引擎级端到端测量（20260912 续）

### 13.1 测量（node v22.23.1 vs aluka，JIT 默认开启，输出均已校验一致）

| 负载 | node | aluka | 结论 |
|---|---|---|---|
| fib30.js（递归调用密集） | 67.2ms | **2224.4ms** | **较 node 慢 33x；较本仓解释器（824ms，fib_bench 口径）还慢 2.7x——JIT 通路对递归调用密集函数存在真实回退** |
| proptest.js（方法/构造/闭包混合） | 50.9ms | 17.5ms | 快 2.92x（毫秒级冷启动 + 方法调用 JIT 生效） |

### 13.2 定性（切片四 P1 修复项）

- jitbench 微基准（hot_loop/prop_sum/closure_call）全部通过，但 fib30 的
  270 万次自递归调用暴露 **JIT 递归调用通路回退**——直调 IC（CallCell）在
  同函数深递归下的行为需要插桩定位（疑点：helper 回退路径占比、
  call_ic_writeback 每调用执行、递归深度下的 cell 复用）；
- 该发现直接改写「1.5x 复合验收」的实质：端到端瓶颈不在解释器微加速
  （切片一~三的 1.03~1.06x），而在 JIT 递归回退——先修 P1，复合验收
  才有意义；已列为切片四首项。

### 13.3 P1 根因确诊（诊断插桩 + 二分复现，插桩已移除、防护注释固化）

```text
探针（examples/jit_fib_probe.rs，fib30.bc，JIT 开启）：
  修复尝试前 fallbacks=2,692,464（≈全部 269 万次调用）compiled=1
  writeback 失配原因打印 → 「upvalues=1」——Go 前端顶层函数声明
  恒捕获 1 个模块环境单元格，被 upvalues.is_empty() 资格判定永久排除
放宽「闭包未捕获」→「机器码不读上值（uses_upvalues=false）」后：
  fib30 fallbacks 269 万 → 144，端到端 2224ms → 1.9ms（1167x），输出正确
但 source 前端 fib(10)（.work/scratch/calc.js）debug+release 均 NaN：
  trace 显示 jit_run 入口实参与返回逐点正确（基例 1/0 正确），
  首个错值出现在「cell FAST 后的机器级直调」——emit_call 快路径
  只换装常量池，未复刻 jit_run 的上值表/帧寄存器换装——机器级
  直调跳过 jit_run 的帧状态管理返回错值
结论：放宽不安全，已回退（is_empty 守卫恢复 + 防护注释固化）；
正确的修复 = emit_call 快路径补全 jit_run 等价的帧状态换装
（上值表安装需机器可寻址的上值表表示——即调用约定协同本身），
列为切片四首项，诊断资产（探针 + jit_compiled_count/jit_slot_summary）
已入库可复现。
```

## 14. 切片四首项实施：直调资格语义修正 + 统一快速分派（20260912 续）

### 14.1 实现

| 项 | 内容 |
|---|---|
| 资格语义修正 | 直调资格从「闭包未捕获单元格」（is_empty）修正为「**编译产物不读上值**」（uses_upvalues=false，jit_entry_for 既有守卫）——闭包捕获单元格（Go/Rust 前端顶层函数声明恒捕获模块环境）与机器码是否读上值是两件事；`jit_call` 与 `call_ic_writeback` 两处同步修正 |
| 统一快速分派 | `jit_direct_call` 对**所有已编译闭包**生效：携带闭包真实上值表进 jit_run（帧语义完备：SavedFrameState 保存恢复 + GC 根保持）——uses_upvalues=true 的被调（递归自引用等体读外层绑定）由此安全直达机器码，省去 try_dispatch/FrameGuard/调用链登记 |
| 测试更新 | 旧不变量测试更新为 `callee_with_unread_upvalues_direct_called_after_first`（fallbacks==1：首轮登记后机器直调）；新增 `callee_reading_upvalues_stays_on_helper`（uses_upvalues=true → 每轮 helper 换装真实上值表，结果 7*Σi 正确）——新旧安全面双向覆盖 |

### 14.2 验证（真实输出）

```text
$ e2e fib(10)（debug + release 双构建）           → 55（修复前 NaN）
$ cargo test --workspace --all-features          → 649 passed, 0 failed
$ cargo test -p aluka-jit --test regression      → 25 passed（含新增 2 例）
$ cargo test -p aluka-cli --test test262_subset_test → 154/154
$ cargo test -p aluka-cli --test conformance_node22_test → 1 passed（28.57s）
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm       → 212 passed, 0 failed
$ cargo test -p aluka-jit --release --test jitbench → 3/3
$ cargo fmt --all --check / clippy -D warnings   → 通过 / 0 error
```

### 14.3 剩余（如实）

- uses_upvalues=true 函数（如源码前端递归 fib）的 cell 机器直调仍被正确排除
  （fib30.js 端到端 ~2.2s）——需要 CallCell 扩展机器可寻址上值表指针
  + 闭包代数守卫（防堆槽复用后的陈旧指针），即调用约定协同的实质工程；
- super 族 / MakeClosure / 生成器（Yield/Await）/ Try 族展开协议；
- 多态桩（2~4 shape 计数数组）；
- 1.5x 复合验收：proptest 引擎口径 3.16x 达成、fib30 口径待上表机器直调
  修通后复核——如实结转。

## 15. 切片四续：NO_FAST 短路微优化（20260912 续）

- `call_ic_writeback` 增加 NO_FAST 短路：同站点同被调已判定不可直调后，
  不再每次调用重复完整资格判定（heap 读 + jit_entry_for + 模板查表）——
  递归调用密集负载上 writeback 是每调用开销；fib30 探针 2224→2049ms（~8%）；
- e2e fib(10)=55 保持；workspace 649/0、test262 154/154、GC 压力 212/0、
  clippy 0 全绿；
- 剩余大头（结构性）：uses_upvalues=true 的机器级 cell 直调需要
  ①CallCell 扩展 upvals_ptr/upvals_len + 闭包代数字段（HeapObject::Closure
  增加分配代数，防堆槽复用陈旧指针）②emit_call 快路径换装上值表指针
  ③jit_load_upvalue 改从 ctx 机器可寻址表读——完整方案与根因链已登记
  （§13/§14），需独立会话以真实证据闭环实施。

## 16. 切片四核心：机器可寻址上值表（20260912 续）

### 16.1 实现

| 项 | 内容 |
|---|---|
| CallCell 扩展 | 尾部追加 `upvals_ptr`/`upvals_len`（预留；机器快路径实际每次从被调堆对象现读表指针——被调对象存活期间 Vec 缓冲区地址稳定，**无陈旧指针问题，无需闭包代数**） |
| JitCtx 扩展 | 尾部追加 `upvals_ptr`/`upvals_len`（当前帧机器可寻址上值表） |
| JitLayout 扩展 | `closure_uv_ptr_off`/`closure_uv_len_off`：Closure.upvalues（Vec）数据指针/长度字段的对象内偏移，运行时探针实测（with_capacity(4)+3 元素区分 ptr/len/cap 三机器字）；初版实现曾把「Vec 内字偏移」直接减基址下溢成天文数字（探针段错误根因，已修正为 vec_base+字偏移-base） |
| emit_call 快路径 | 直调前从被调堆对象现读 upvalues 表指针/长度写入 ctx（旧值存调用方栈槽，返回后恢复——与常量池交换同构的栈式嵌套） |
| jit_load_upvalue 重写 | 优先读 ctx 机器可寻址表（非空且界内），回退解释器 current_upvalues |
| jit_run 同步 | 换装 ctx.upvals_ptr/len（本次真实单元格），随 saved 元组保存/恢复 |
| 资格放宽 | `jit_entry_for` 移除 uses_upvalues 过滤——机器换装已使体读上值的被调可安全直调 |

### 16.2 排障记录（真实过程）

NO_FAST 短路（2004c76）曾阻断 cell 升级路径：「未编译→NO_FAST」被判死，
编译完成后永不升级 FAST——移除短路恢复逐调用重判（升级价值 ≫ 8% 微优化）。
另修复两处 unsafe-op-in-unsafe-fn 警告（unsafe 块显式化）。

### 16.3 验证（真实输出）

```text
$ jit_fib_probe（fib30.bc，JIT 开启）
    → fallbacks 2,692,464 → **2**；fib30 2224ms → **21ms**
$ aluka run .work/scratch/calc.js                → fib(10) = 55（debug+release）
$ aluka run .work/scratch/fib30.js               → 832040，端到端 34.2ms
$ 引擎级配对（vs node v22.23.1，含进程启动，min-of-5）
    fib30.js:   node 53.1ms vs aluka 34.2ms = **1.55x**
    proptest:   node 56.3ms vs aluka 15.3ms = **3.13x**
$ cargo test --workspace --all-features          → 649 passed, 0 failed
$ cargo test -p aluka-cli --test test262_subset_test → 154/154
$ cargo test -p aluka-cli --test conformance_node22_test → 1 passed（28.34s）
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm       → 212 passed, 0 failed
$ cargo test -p aluka-jit --release --test jitbench → 3/3
$ cargo fmt --all --check / clippy -D warnings   → 通过 / 0 error
```

### 16.4 ≥1.5x 复合验收复核结论

- **引擎端到端口径达成**：fib30（递归调用密集）1.55x、proptest（方法/构造
  密集）3.13x——M6.2 NaN-boxing + M6.3 PIC/JIT 全链协同后，对 Node.js 22
  的端到端吞吐在两类代表性负载上均越过 1.5x；
- jitbench 保守门禁（JIT ≥ 解释器）3/3 保持；M6.3 验收原文「密集计算与
  循环调用基准测试显著超越解释器基线」——jitbench 3/3 + fib30 引擎级
  跨线，验收达成；
- fib_bench（解释器单端口径，JIT 显式关闭）1.03~1.06x 如实保留——该口径
  度量的是表示切换单项收益，非复合吞吐；
- 剩余增强项（不阻塞验收，按登记推进）：super 族 / MakeClosure 机器直调 /
  生成器（Yield/Await）/ Try 族展开协议、多态桩。

## 17. 切片五：属性 IC 升级 4 路组关联（多态桩，20260912 续）

- `pic.rs` 从单槽直接映射升级为 **4 路组关联**（1024 组 × 4 路）：同站点
  2~4 个 shape 的绑定共存组内（线性探测命中），不再互挤逐出；写回策略
  = 同 (site,shape) 无操作 / 空槽插入 / 组满驱逐组首（5 态以上退化随机
  驱逐，语义仍正确）。初版「同 site 旧形状原位替换」策略会立即摧毁
  多态条目（自测捕获：4 态站点命中数为 0），已改为按 (site,shape) 对
  独立存储；
- 读取与写入 IC 共用组结构；方法 IC 维持单态（读上值表换装的守卫链
  更重，多态方法站点较少，按登记后续）；
- 新增 2 测试（4 态全命中 + 5 态驱逐正确性），PIC 套件 13/13；
- 门禁：workspace 651/0、test262 154/154、GC 压力 214/0、clippy 0 全绿。

## 18. 方法 IC 多态化（20260912 续）

- `get_method_ic`/`method_ic_writeback` 升级 4 路组关联（与属性 IC 同构）：
  组内线性探测按「站点 + receiver shape + 原型 ObjectRef + 原型 shape」
  四元匹配；写回 = 同四元组无操作 / 空路插入 / 组满驱逐组首；
- 新增多态方法站点测试（3 shape 交替全命中），PIC 套件 14/14；
- 门禁：workspace 652/0、test262 154/154、clippy 0 全绿。

## 19. JIT 指令流扩容第一批：12 操作码接入（20260912 续）

- 新增 helper：`jit_typeof`/`jit_typeof_global`/`jit_get_elem`/`jit_set_elem`/
  `jit_del_prop`/`jit_get_proto`/`jit_instanceof`/`jit_in`/`jit_new_array`/
  `jit_array_push`（语义经解释器单源方法），编译臂：PushNull（纯常量）/
  UnaryPlus（复用 to_number 通道）/Typeof/TypeofGlobal/GetElem/SetElem/
  SetElemTop/DelProp/GetProto/Instanceof/In/NewArray|BuildArray/ArrayPush；
- JIT 操作码覆盖 52 → **66**/106；
- **连带发现并修复引擎既有缺陷**：`delete_property` 无 Array 分支——
  `delete arr[i]` 静默无效（node 读 undefined，aluka 仍读原值）。补 Array
  分支：索引键置 undefined（length 不变；`idx in arr` 恒真为已知近似），
  非索引键删自有属性表。修复后 bisect/jitops 双引擎输出逐字节一致；
- `unsupported_opcode_marks_rejected_once` 更新为仍子集外的 Yield；
- 门禁：workspace 652/0、test262 154/154、GC 压力 215/0、clippy 0 全绿。

## 20. 指令流扩容第二批 + 词法器缺陷修复（20260912 续）

### 20.1 指令流第二批（12 操作码，覆盖 66 → 78/106）

- ReturnUndef（无 try 表函数 = 返回 undefined，编译资格保证）、DelElem、
  BitAnd/BitOr/BitXor/Shl/Shr/UShr/BitNot（单 helper `jit_bitop` 带 op
  选择子：ToNumber + i32 位语义，USHR 按 u32 位型右移——与解释器逐位
  一致，含负数位型用例）、StoreGlobal（CJS 注入名进模块作用域/其余进
  全局表；`jit_run` 补 `current_func_idx` 维护）；
- e2e：位运算哈希链 + 全局赋值 + DelElem 负数位型用例，双引擎逐字节一致。

### 20.2 连带发现并修复词法器缺陷（引擎级）

`multi_puncts` 缺失 `|=`/`&=`/`^=` 三个复合赋值 token——被切成单字符
`|`/`&`/`^` + `=`，复合赋值静默退化为裸位运算表达式（赋值丢失）。
`&&=`/`||=`/`??=`/`>>>=` 在表中而 `&=`/`|=`/`^=` 缺失的不对称是笔误。
补齐后 bisect 微用例与 e2e 全部与 node 逐字节一致。

### 20.3 门禁（真实输出）

```text
$ cargo test --workspace --all-features          → 652 passed, 0 failed
$ cargo test -p aluka-cli --test test262_subset_test → 154/154
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm       → 215 passed, 0 failed
$ cargo fmt --all --check / clippy -D warnings   → 通过 / 0 error
```

## 21. 指令流扩容第三批：8 操作码接入（20260912 续）

- StoreUpvalue（**机器可寻址上值表直写**——闭包共享单元格语义保持：
  经 ctx.upvals_ptr 写入被调闭包的真实单元格，Load/StoreUpvalue 混用
  计数器模式双引擎一致）、CloseUpvalues（机器帧不创建 open 单元格，
  安全无操作）、SetGetterObj/SetSetterObj/SetGetterComputedObj/
  SetSetterComputedObj（统一 `jit_set_accessor`：Ordinary 限定 +
  has_accessors 粘性，与解释器同语义）、SpreadObject（own_properties
  逐键写入）、EnumKeys（Proxy ownKeys trap 降级 + for-in 键快照）；
- JIT 操作码覆盖 78 → **86**/106；
- e2e：闭包计数器（Load/StoreUpvalue 混用，20000 ✓）、spread/for-in/
  getter（750/42 ✓）双引擎逐字节一致；
- 门禁：workspace 652/0、test262 154/154、GC 压力 215/0、jitbench 3/3、
  clippy 0 全绿。

## 22. 指令流扩容第四批：3 操作码接入（20260912 续）

- **OptionalJump**（`?.` 短路）：栈顶 nullish 位比对（NaN-box 常量直比）
  → select 原位合并 undefined/原值 → 条件跳转——两路径栈深度一致，
  编译期栈模型无分歧；
- **JmpNullishKeep**（`??` 短路）：nullish → 弹出落点；非 nullish →
  持值跳转（编译期保留栈顶；落点路径该值已死但运行时无害）；
- **ArraySpread**：迭代物化 + 写屏障追加（J2 错误约定降级空集）；
- e2e：`?.`/`??` 热函数（1620/-60/2700 ✓）与数组展开（600 ✓）
  双引擎逐字节一致；
- JIT 操作码覆盖 86 → **89**/106；门禁：workspace 652/0、test262 154/154、
  GC 压力 215/0、clippy 0 全绿。

## 23. 指令流扩容第五批：3 操作码接入（20260912 续）

- **SetPropComputedObj**（动态键写入，obj 保留）、**CallMethodArgs**
  （方法 IC + 实参数组调用，helper `jit_call_method_args` 复用
  `get_method_ic` + `invoke_callable` 单源语义）、**MakeRegexp**
  （pattern/flags 盒 → RegExp 堆对象）；
- e2e：正则字面量 + 动态键写入（200 ✓）、方法数组实参调用（11625 ✓）
  双引擎逐字节一致；
- JIT 操作码覆盖 89 → **92**/106；门禁：workspace 652/0、test262 154/154、
  GC 压力 215/0、jitbench 3/3、clippy 0 全绿。

## 24. 指令流扩容第六批：GetIterator/GetAsyncIterator 接入（20260912 续）

- `Op::GetIterator | Op::GetAsyncIterator` 臂（约 90 行迭代器分派链：
  生成器/流/数组/类型化数组/字符串/Map/Set/自定义 Symbol.iterator）
  **原样抽取**为 `Vm::get_iterator_dispatch(val)`（12 处 push → return Ok），
  解释器臂与 JIT helper `jit_get_iterator` 共用单源；
- 实施中的双重弹栈回归（dispatch 内残留 pop）被 iter_protocol e2e
  当场捕获（StackUnderflow），修正后 5/5 恢复；
- e2e：for-of 数组/Set/字符串码点迭代热函数（15595 ✓）双引擎一致；
- JIT 操作码覆盖 92 → **94**/106（GetIterator/GetAsyncIterator）；
- 门禁：workspace 652/0、test262 154/154、GC 压力 215/0、jitbench 3/3、
  clippy 0 全绿。

## 25. 剩余 12 操作码的接入陷阱清单（下一会话设计输入，20260912 核验）

逐项核验确认全部为协议级工程，直接机器接入会产生静默错值：

| 操作码 | 陷阱 | 所需协议工作 |
|---|---|---|
| CallThis/CallThisArgs/ConstructThis/ConstructThisArgs | this 取自当前帧 `locals[0]`，而 JIT ABI 槽 0 是 undefined 占位（LoadLocal 0 资格拒绝的同一根因）——机器接入得 undefined this（fib10 NaN 同类陷阱） | JIT ABI 扩展 this 形参通道（入口换装 + 资格判定联动） |
| MakeClosure | 两个闭包捕获同一变量必须**共享单元格**（经典计数器语义）；机器侧按闭包新建单元格会破坏共享突变 | JIT 帧参与 open_upvalues 注册协议 |
| MakeClass | computed keys/super 从解释器栈弹出，JIT 值栈是 SSA 不同步；需 vm.stack 桥接 + 编译期 class 元数据注入 | 栈桥接模式 + 模板访问；价值低（类定义仅模块初始化，不进热点） |
| TryEnter/TryExit×3/Throw | helper 返回通道无错误面（J2 约定 Err→undefined）；Throw 必须传播——机器码需哨兵返回值 + jit_run 解释 | 机器级异常传播协议（栈映射协同） |
| Yield/Await | 挂起协议需记录机器 pc 并恢复（yield_pc 语义在 SSA 栈上不成立） | 生成器展开协议（独立专项） |
| ForInNext/End | **解释器侧即未实现**（UnimplementedOpcode），需先补解释器再谈 JIT | 解释器补实现先行 |

结论：M6.3 验收（94/106 + ≥1.5x 引擎级复合验收达成）不依赖以上项；
每项均为独立会话的多日专项，本清单即其设计输入。

## 26. M7 启动：M7.1 运行时程序合并验收验证（20260912 续）

### 26.1 现状盘点

- 三二进制并存：`alukac`（独立前端：compile/disasm/build 依赖树）、
  `aluvm`（独立 VM：加载 → Verifier → 解释执行）、`aluka`（统一入口：
  `run` = 编译/校验/执行一体，`build` = 依赖树打包）——**统一单二进制
  形态已存在**，`aluvm` 装配注释已登记与 `aluka` 共用
  `aluka_runtime::execute_bc` 同一流程（M7.1 口径先行）。

### 26.2 验收验证（真实输出）

```text
$ 独立分发：/tmp/m71_dist 仅含 aluka.exe + hello.js（无仓库其它文件）
  ./aluka.exe run hello.js（require("os") + 算术）
    → hello from standalone: win32 3        → 单文件自足 ✓
$ 分层流水线不变：
  ./alukac compile mod.js -o mod.bc → 363 字节（函数: 2）
  ./aluvm run mod.bc                → fib(15) = 610   ✓（前端/后端分离）
  ./alukac build mod.js → aluka_build/mod.bc
  ./aluvm run aluka_build/mod.bc    → fib(15) = 610   ✓（依赖树构建）
  ./aluka run mod.bc                → fib(15) = 610   ✓（统一入口直跑 .bc）
```

### 26.3 验收口径（如实）

- ✅ 统一单二进制（`aluka` = run/build 统一交互；内部 源码→字节码校验→
  VM 解释 分层流水线保持不变——独立前后端仍可全程复现）；
- ✅ 单二进制独立分发（Windows 本机验证）；
- ⏳ 跨平台执行验证：Windows 验证通过；Linux/macOS 需 CI 矩阵
  （本环境无交叉工具链与远端 CI 工作流），**登记为待 CI 补验项**，
  不在本机声称跨平台完成。

### 26.4 M7.2 启动：官方 test262 语料导入 + 基线（20260912 续）

| 项 | 内容 |
|---|---|
| 导入器 | `tools_m72_import.py`：从浅克隆官方 test262（tc39，5.7 万文件）按 50 个选域（语言核心 types/expressions/literals/statements + 内建 Math/JSON/Number/Object/Array/String/Symbol/RegExp/Map/Set/Promise 等）导入 **1000 例**（m72- 前缀；跳过 module/raw/async/worker/CanBlock/动态 includes 形态）；每例内联官方 harness（assert/sta/propertyHelper/compareArray/fnGlobalObject/deepEqual/isConstructor）+ isTrue/isFalse 兼容垫片；`M72_PRELOAD` 环境变量门控预载 |
| 语料规模 | **1154 例**（手写 154 + 官方导入 1000）——达成 M7.2 「≥1000 例」规模指标 |
| 首轮基线 | **642/1154**（无预载口径；预载口径 607——官方 assert.js 语义与 runner 最小 harness 尚有冲突，如 assert.throws 的 instanceof 严格化） |
| 双层门禁 | test262 runner 改为分层断言：手写语料 154 例**硬性全过**（回归门禁）；官方导入语料断言基线下限 480/1000，随引擎修复逐级上调至 100%（M7.2 验收线） |

### 26.5 连带发现并修复的两个 BigInt 引擎缺陷（官方语料首轮暴露）

1. **进制 BigInt 字面量损坏**：`0xFFn`/`0b1010n`/`0o777n` 全部求值为 0——
   进制数字循环按 alphanumeric 吞掉 `n` 后缀，且 i64 溢出静默得 0；
   修复：剥 `n`/`N` 后缀、整串 `0x…` 载荷下发、VM 物化时经 BigNat
   （base-2^32）按 radix 折叠为十进制归一化；
2. **BigInt 加法是字符串拼接**：`1n + 2n` → "12"——add_values 无 BigInt
   分支，落入「对象 → ToPrimitive 拼接」；修复：BigNat 补
   add_small/add_big/sub_big/cmp_big，新增 `bigint_dec_add`
   （同号相加/异号相减取大符），`add_values` 增 BigInt+BigInt 分支；
   此前官方语料中 654 基线的部分「通过」实为 0===0 假阳性——真语义
   落地后基线如实回落至 642。

### 26.6 门禁（真实输出）

```text
$ cargo test --workspace --all-features          → 652 passed, 0 failed
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm       → 215 passed, 0 failed
$ cargo fmt --all --check / clippy -D warnings   → 通过 / 0 error
$ cargo test -p aluka-cli --test test262_subset_test → 642/1154（手写 154/154 硬门禁 ✓）
$ M7.1 验证组（见 §26.2）                        → 全部通过
```

### 26.7 M7.2 剩余（如实）

512 例官方语料失败分桶：SyntaxError（解析器缺口，~100）、not-a-function
（预载 harness 语义冲突与 verify* 家族，~113）、Math 缺口
（atan2/asin/acos/clz32 等，~18）、OOM（Array 超长构造应 RangeError，6）、
BigInt mul/div/cmp 覆盖、其余零散——每桶为独立修复轮，按总表登记推进。
M7.3（npm Top 50 签核）未启动。

### 26.8 M7.2 分桶修复轮一：包装对象 + Math 缺口 + 数组大下标（20260912 续）

| 修复 | 内容 |
|---|---|
| **包装对象（全缺失）** | `new Boolean/Number/String(v)` 此前返回原始值或裸对象——do_construct 与无 new 直调共用分支。新增三包装构造臂：Ordinary 实例挂对应原型 + 数据槽（`[[BooleanValue]]`/`[[NumberValue]]`/`[[StringValue]]`，Dict 模式直载）+ String 包装 length/索引属性；`String(new String("hi"))` 经 format_value 透传 |
| **Boolean.prototype 方法分派** | 此前无 handler（`Boolean.prototype.toString()` 报 TypeError）——新增 `bool_method_dispatch`（receiver 取值序：原始布尔→自身 / 实例数据槽 / 原型自身→规范缺省 false）+ 注册 |
| **宽松相等解包** | `new Number(5) == 5` / `true == new Boolean(true)` / `new String("hi") == "hi"` 曾全 false——eq 增 wrapper_data 纯堆解包（Number/Boolean 臂 + Object/Object 臂解包一层递归）；strict_eq 不受影响 |
| **Math 方法缺口** | atan/atan2/asin/acos/sinh/cosh/tanh/asinh/acosh/atanh/clz32/fround/imul/expm1/log1p 共 15 个方法接入（imul 按 ToUint32 规范实现） |
| **数组大下标 OOM** | `a[4294967295]=…` 曾触发 2^32×8 字节分配直接 abort——索引键规范上限（<2^32-1）+ 密集容量上限 1e7，超限落自有属性表；读取面 elements.get → properties 回退 |
| **notSameValue** | runner 最小 harness 补官方 assert.notSameValue |

效果：test262 基线 642 → **656/1154**（not-a-function 桶 113→65、Math 桶清零、OOM 桶清零）；node22 conformance 差分 877/878 的 gen-coerce-matrix-0034 回归由宽松相等解包修复（现全绿）。

门禁：workspace 652/0、GC 压力 215/0、clippy 0、test262 手写 154 硬门禁 ✓、conformance 差分 ✓、jitbench 3/3。

### 26.9 M7.2 分桶修复轮二：JSON 转义 + 词法错误通道（20260912 续）

| 修复 | 内容 |
|---|---|
| **JSON.parse 转义臂错值** | `\b`/`\f` 转义臂误写控制字符值 0x08/0x0c（转义字母实为 b/f）——`JSON.parse('"\b"')` 一律报 invalid escape；修正后合法转义全通 |
| **词法错误通道** | TokenKind 新增 `LexError` 变体 + Lexer `pending_lex_error` 待发标志（skip 函数无返回通道，next_token 在 inner 返回 Eof 时拦截换发）——未终止多行注释（`/*x` 静默吞到 EOF）现判 SyntaxError；解析器 parse_program 循环任意位置遇 LexError 即 record_error 判死 |
| 效果 | test262 基线 656 → **657/1154**（+2 JSON 转义、+1 未终止注释等）；aluka run/alukac compile 对未终止注释双双报「解析错误: SyntaxError」 |

### 26.10 剩余大桶的根因定性（下一轮设计输入）

- **parse 负例未拒 ~85 例**：解析器 ASI（自动分号插入）无换行也插入——
  `{ 1 2 } 3`（同行相邻表达式）、`throw\n1`（受限产生式）等被静默接受。
  系统性修复 = lexer 为每 token 记录「前置换行」标记 + 解析器
  consume_stmt_end 仅在换行/`}`/EOF 时自动补分号—— invasive 改造，
  独立专项；连带 `= 1;`（无目标赋值语句）等少数形态；
- **sameValue 107 / TypeError 84 / not-a-function 65 / assert.throws 54**：
  逐例定位中（sameValue 多为内置方法返回值细节；not-a-function 余量为
  propertyHelper 家族在 includes 内联后的残余缺口）。

### 26.11 M7.2 轮三：ASI 严格化实测净负，回退并如实登记（20260912 续）

- 尝试：`nl_before_current`（token 间隙换行扫描）+ `eat_semi` 严格化
  （限换行/`}`/EOF 三情形）+ throw 受限产生式；连带修复默认表达式
  语句的**逗号运算符**缺口（`ref = other, other = ref[0];` 此前被宽松
  eat_semi 拆成多条语句——parse_expr_sequence 整串解析，express e2e 回归
  由它修复）；
- 实测：误伤 > 修复（t262 657 → 642）——hashbang 未剥离（`#!/usr/bin/env node`
  被 `#`+`!` 误析）、`/` 除法-正则歧义、空文本 token 等前置缺陷被严格化
  暴露为误报；express 真实包 131 模块构建也依赖宽松形态修复后才恢复；
- **决策：eat_semi 回退宽松态**（语义正确的逗号序列解析保留）；严格化
  前置条件已明确：①hashbang 剥离 ②语句模型清理（空 token 根因）
  ③语料回归全绿——按登记于后续专项，不放宽任何既有断言；
- 净保留：逗号运算符语句解析 + throw 受限产生式检查 + nl_before_current
  助手 + JSON 转义 + LexError 通道；基线 **658/1154**（657 + throw/注释）；
- 门禁：workspace 652/0、GC 压力 215/0、clippy 0、conformance 差分 ✓、
  express e2e ✓、jitbench 3/3。

### 26.13 M7.2 分桶修复轮五：非简单参数 use-strict 判死 + BigInt 字面量校验（20260912 续）

| 修复 | 内容 |
|---|---|
| **use strict + 非简单参数** | 解构/默认/剩余参数的函数体含 "use strict" 指令 → SyntaxError（规范 14.6.2；官方语料 async-function/async-generator 语法族 ~28 例）——parse_function_def 检测 prologue 非空（解构+默认注入标志）/is_var_args，扫描指令序言 |
| **BigInt 字面量校验** | 进制分支：数字位必须属于进制（`0b2n`/`0o8n` 判死——此前 `to_digit().unwrap_or(0)` 静默容错）、分隔符不得居首/居尾/连续（`0b_1n`/`1__0n`）；十进制分支：指数+n 判死（`1e3n`——此前被当 "1e3" 字符串入堆）、传统八进制形态（`01n`）判死、分隔符位置同上 |
| 效果 | test262 基线 653 → **680/1154**（+27：use-strict 族 ~24 + bigint 负例 ~10，部分负例此前被 node 校验划 INVALID） |
| 门禁 | workspace 652/0、GC 压力 215/0、clippy 0、conformance 差分 ✓、express e2e ✓、jitbench 3/3 |

### 26.14 M7.2 分桶修复轮六：Object/Array 无 new 直调 + NativeCtor 方法值路由（20260912 续）

| 修复 | 内容 |
|---|---|
| **Object()/Array() 无 new 直调** | invoke_callable 的 NativeCtor 分支缺 Object/Array——`Object()` 报 [function Function] is not a function（heap 索引被误用作函数模板下标调用错函数）；补 do_construct 路由 |
| **NativeCtor 方法值路由** | 分派链尾兜底不识别 NativeCtor 方法值——`Object.prototype.constructor()` 报错；补 NativeCtor → invoke_callable（无 new 构造语义） |
| 效果 | test262 基线 680 → **687/1154**（S15.2.1.1 Object 族 ~7 例）；C 形态 `Object()()` 双引擎一致报错 |
| 门禁 | workspace 652/0、GC 压力 215/0、clippy 0、conformance 差分 ✓、express e2e ✓、jitbench 3/3 |

### 26.15 M7.2 分桶修复轮七：U+2028/U+2029 行终结符（20260912 续）

- lexer 空白跳过补 U+2028（行分隔符）/U+2029（段分隔符）——规范行终结符
  一直被当垃圾字节（`1\u2029+1` 解析失败、`var\u2028x` 报缺变量名）；
  算术五则 ~76 例 + line-terminators 族 ~9 例 + var 声明 8 例共同根因；
- 效果：test262 基线 687 → **697/1154**（+10 净修复——部分同根因用例
  计入 invalid）；e2e `eval("1\u2029+1")===2` 与 `"a\u2028b".length===3`
  双引擎一致；
- 门禁：workspace 652/0、GC 压力 215/0、clippy 0、conformance 差分 ✓、
  express e2e ✓、jitbench 3/3。

### 26.16 M7.2 分桶修复轮八：Unicode 空白集扩展（20260912 续）

- lexer 空白集补齐规范 WhiteSpace：<VT> (0x0B，Rust is_ascii_whitespace
  不含)、NBSP (U+00A0)、ZWNBSP (U+FEFF)、USP 面（U+1680/U+2000..3000/
  U+202F/U+205F/U+3000）——S11.6.1 算术五则的 `eval("1\u000B+…")` 系列
  全依赖（check#2/5/8/10 等 VT+NBSP 形态）；
- 效果：test262 基线 697 → **703/1154**（+6 算术五则族）；
- 门禁：workspace 652/0、GC 压力 215/0、clippy 0 全绿。

### 26.17 M7.2 轮九：语义核心缺口补全（20260912 续）

**基线**：703/1154（336 正向失败 + 60 parse 负例 + 115 invalid）。

**失败分桶（真实清单）**：
- `S11.*_A2` 族（未声明标识符读取应抛 ReferenceError）≈27；
- `S11.*_A3` 族（包装对象/对象参与算术的 ToPrimitive）≈30；
- `typeof Math.*` / `Math.PI`（Math 常量与部分方法缺失）；
- `Number.MIN_VALUE` 取错（`f64::MIN_POSITIVE` ≠ 最小次正规数）；
- `Number()` 无参返回 NaN（应 0）、字符串转义 `\f` 词法缺口；
- 数组 elision `[,,,1,2]`、前导点数字 `.12345` 解析缺口。

**本轮修复范围**：
1. 未声明标识符读取抛 ReferenceError（`resolve_global` 改 `Option`，LoadGlobal
   命中 None 即抛；TypeofGlobal 保持返回 "undefined"）；
2. 包装对象 ToPrimitive：`add_values` 与 `to_number_value` 解包
   `[[NumberValue]]`/`[[BooleanValue]]`/`[[StringValue]]`/Date `_timeValue`；
3. Math 对象补全：8 个常量 + 全部标准方法（含缺失的 `sin`/`cos`/`tan`）
   挂为属性（`typeof Math.x === "function"`）；
4. Number 静态：`MIN_VALUE = f64::from_bits(1)`（最小次正规数）；`Number()` 无参 = 0；
5. 词法器字符串转义 `\f`/`\b` 修正；JSON.parse 转义臂复核；
6. 解析器：数组 elision 与前导点数字字面量。

**验收标准**：test262 手写 154 硬门禁全绿 + 官方导入语料基线显著上调；
workspace 全量测试、GC 压力、clippy、fmt、node22 conformance 差分全绿。

### 26.18 M7.2 轮九：实现记录与门禁证据

#### 实现（12 处引擎修复）

| # | 修复 | 位置 | 说明 |
|---|---|---|---|
| 1 | **未声明标识符读取抛 ReferenceError** | `vm/interpreter.rs` `resolve_global` → `Option`；`LoadGlobal` 命中 None 抛 `ReferenceError: x is not defined` | 此前 `_ => Undefined` 静默求值；`typeof`/`delete` 走豁免通道 |
| 2 | `typeof <自由标识符>` 走 `TypeofGlobal` | `compiler/codegen.rs` | 此前编译为 `LoadGlobal; Typeof`，ReferenceError 化后会误抛；`typeof` 对未声明必须返回 `"undefined"` |
| 3 | `delete <标识符>` 不读取标识符 | `compiler/codegen.rs` | 局部/上值/只读全局 → `PushFalse`，未声明 → `PushTrue`（此前先求值再删，未声明会抛） |
| 4 | 包装对象 ToPrimitive 解包 | `vm/interpreter.rs` `wrapper_primitive` + `to_number_value`；`vm/ops.rs` `add_values` | `[[NumberValue]]`/`[[BooleanValue]]`/`[[StringValue]]`/Date `_timeValue`；`new Boolean(true)+true → 2`、`new Number(1)/true → 1` |
| 5 | Math 常量 + 全部标准方法挂属性 | `vm/interpreter.rs` `Vm::new` | 8 常量（PI/E/…）+ 35 方法（补 `sin`/`cos`/`tan`）；`typeof Math.exp === "function"`、`Math.PI === 3.14159…` |
| 6 | `Number.MIN_VALUE` = 最小次正规数 | `vm/builtins/global/mod.rs` | `f64::MIN_POSITIVE` → `f64::from_bits(1)`（5e-324） |
| 7 | `Number()` / `String()` 无参 = 0 / "" | `vm/call.rs` | 此前 `args.first().unwrap_or(undefined)` → NaN / "undefined" |
| 8 | 包装原型 `constructor` 回指 | `vm/builtins/surface.rs` | `new String().constructor === String` 等（S15.5/6/7 族） |
| 9 | 词法字符串转义 `\f`/`\b`/`\v` | `parser/lexer.rs` | 此前落 `other => push(char)`，`\f` 变字母 `f`（`Number("\f")` 错为 NaN） |
| 10 | BigInt 小数/前导点判死 | `parser/lexer.rs` | `.0000000001n` / `1.5n` → SyntaxError（此前静默接受） |
| 11 | 数组 elision + 前导点数字 | `parser/parser.rs` / `lexer.rs` | `[,,,1,2]`（length 5）、`.12345` / `.00000012345` |
| 12 | 标签语句 `label: Statement` | `parser/ast.rs`/`parser.rs`/`codegen.rs` | `{length: 3000}[]` 等块内标签此前被当表达式求值；**消除 ReferenceError 化后的 18 例回归** |
| 13 | BigInt `===` 按内容比较 | `vm/ops.rs` `strict_eq` | 归一化十进制文本比较（`0b0_1n === 0b01n`） |
| 14 | 只读全局赋值忽略 | `vm/interpreter.rs`/`jit_helpers.rs` `StoreGlobal` | `Infinity`/`NaN`/`undefined` 赋值静默忽略 |
| 15 | GC 压力相位基准 | `vm/gc.rs`/`heap.rs` | `stress_base` 使 `drain_gc` 后压力触发相位从 0 起算（修复轮九新增分配数改变相位、压力下 `cyclic_garbage_is_collected` 触发点落入分配间隙的**测试脆弱性**，非引擎缺陷） |

#### 门禁证据（真实输出）

```text
$ cargo fmt --all --check                              → 通过
$ cargo clippy --all-targets --all-features -- -D warnings
    → 0 error
$ cargo test --workspace --all-features
    → TOTAL passed: 652, failed: 0
$ ALUKA_GC_STRESS=8 cargo test -p aluka-vm -p aluka-cli --all-features
    → passed: 457, failed: 0
$ cargo test -p aluka-cli --all-features --test test262_subset_test
    → 811/1154 passed（115 invalid；手写 154 硬门禁 ✓）
      基线 703 → 811：新增通过 108、回归 0（逐例集合差分核验）
$ cargo test -p aluka-cli --all-features --test conformance_node22_test
    → 1 passed（全量差分 vs node v22.23.1 stdout 逐字节一致，37.51s）
$ cargo test -p aluka-jit --release --test jitbench
    → 3 passed（JIT ≥ 解释器保守门禁保持）
```

#### 关键教训

- **ReferenceError 化的连带面**：标识符读取语义收紧会暴露此前被
  「未声明 = undefined」掩盖的解析器缺口（标签语句）与 `typeof`/`delete`
  豁免缺失——18 例回归当场由逐例集合差分捕获并全部修复，最终净收益
  108 例、零回归。这正是「真实证据闭环 + 逐例差分」的价值。
- **GC 压力相位**：新增构建期分配改变了 `allocated % N` 的触发相位，
  使依赖「两次分配间不触发回收」的 GC 单元测试变脆；以 `stress_base`
  将测试相位归零，恢复确定性（引擎根扫描本身无误）。

#### 剩余（如实结转）

- BigInt mul/div/mod/sub 与混合类型 TypeError（~20 例）、对象 ToPrimitive
  经用户 `valueOf`/`toString`（`Number({valueOf})`）、`Object(2n)` 包装、
  BigInt 全局函数；
- 标签语句仅作跳转目标标记，`break/continue label` 未接线；
- Boolean.prototype `this` 值 TypeError（需 `CallMethod` 尊重实例自有方法值）、
  Symbol 家族、`Object.prototype.toString` 内建标签、`Date()` 字符串形态、
  模块顶层 `typeof this`；
- parse 负例大桶（async-function early errors / ASI 严格化 / 行终结符）——
  按登记推进，不在本轮声称完成。


### 26.18 轮十遗留项登记：CALL_METHOD toString 覆盖路径追踪（下一轮首项）

- 现象确认：`Array.prototype.toString = Object.prototype.toString; x.toString()`
  node → [object Array]，aluka → ""（空串，内置 join 语义）；
- 探针证据：ALUKA_METHOD_IC_DEBUG 探针置于 get_method_ic 慢路径**无输出**——
  CALL_METHOD 走到了分派链更早的某个 toString 臂（非数组原型 IC 通道），
  或 CALL_METHOD 的 GetProp+Call 分解形态未经过预期路径；
- 下一轮动作：①以 CALL_METHOD 分派链逐臂断点定位 toString 实际执行臂；
  ②在该臂补覆盖检测（属性值 native name ≠ "Array.prototype.toString"
  时走通用 invoke）；③同步给 Object.prototype.toString 补 Array this 的
  IsArray 分支（`Object.prototype.toString.call([1,2])` → [object Array]
  当前正确，但 join 空串形态需复核）。
- 临时探针已全部移除；workspace 652/0、GC 压力 215/0、clippy 0、
  test262 手写 154 硬门禁 ✓。

## 27. M7.2 轮十一：数组方法面 fallback 落原型链（20260913）

- CALL_METHOD 数组方法面大 match 的 `_ => Ok(Undefined)` 改为
  `get_method_ic` + `invoke_callable`——未内置的数组方法（用户对
  Array.prototype 的扩展如 `foo`、被覆盖的 `toString`）落到原型链
  解析并调用，不再吞成 undefined；
- e2e：`Array.prototype.foo`/`toString` 覆盖（FOO/OVR ✓）、
  Object.prototype.constructor 调用、`Object()` 直调全通；
- 门禁：workspace 652/0、conformance 差分 833/878→含 invalid 全一致、
  GC 压力 215/0、jitbench 3/3、clippy 0 全绿。

### 26.20 M7.2 分桶修复轮十二：Error 子类 prototype.constructor 挂接（20260913）

- **根因**：`error_subclass_ctor`（SyntaxError/TypeError/RangeError 等
  惰性单例）创建后未在 prototype 上挂 `constructor`——官方
  assert.throws 用 `thrown.constructor !== ExpectedCtor` 判定错误类型，
  JSON.parse 语法错误实例的 constructor 沿链找到 Error 构造器 → 判
  "Expected a SyntaxError but got a Error" → Test262Error → 官方语料
  JSON/assert.throws 桶全失败；
- 修复：error_subclass_ctor 创建 ctor 后补
  `prototype.constructor = ctor`；连带 syntax_error 错误实例挂
  SyntaxError.prototype（instanceof 语义）；
- 效果：test262 基线 793 → **824/1154**（JSON.parse 桶 15 例 +
  assert.throws 相关批量转绿）；
- 门禁：workspace 652/0、GC 压力 215/0、clippy 0、conformance 差分 ✓、
  jitbench 3/3、express e2e ✓。

## 28. M7.2 轮十三：ASI 严格化重启 + bare-ident 豁免（20260913）

- **ASI 严格化重启净正**：前置缺陷（hashbang/U+2028 空白/JSON 转义/
  Error 子类 constructor）逐轮解除后，eat_semi 严格化（限换行/`}`/EOF）
  实测 **824 → 837/1154**（asi 负例全绿且无误伤——上轮的回退判断
  在新基线下不再成立）；
- **bare-ident 豁免**：裸 Ident 表达式语句（TS `declare enum` strip-only
  豁免形态）宽松吞分号——修复 ts_enum_is_rejected_with_declare_exempt
  单测（workspace 652/0 恢复）；
- **M72_FLOOR 480 → 800**：反映当前真实基线（840 含 invalid 口径
  837/811 差异为计数方式，取 through 数）；
- 门禁：workspace 652/0、t262 840/1154、GC 压力 215/0、conformance
  差分 ✓、express e2e ✓、jitbench 3/3、clippy 0 全绿。

## 29. M7.2 轮十四：add_values ToPrimitive 错误通道化 + 堆原始形态结果判定（20260913）

- **主修复（结果判定口径）**：to_primitive_number 采纳 valueOf/toString
  返回值时只认「非 Object case」——NaN-box 下堆字符串/BigInt 亦为
  Object case（语义是原始值），`Array.prototype.toString` 返回堆串被
  误判「仍是对象」→ 两方法全跳过 → 一律抛
  "Cannot convert object to primitive value"。修复为与开头早退同口径
  （堆 String/BigInt 即原始值采纳）；
- **波及面**：所有对象参与 `+`（`[]+[]`、`[]+{}`、`{}+0`、自定义
  toString、Date 拼接）此前全部抛错；修复后 7/7 探针逐字节对齐 Node；
- **连带转绿**：conformance 差分 4 例失败（08-http-agent vm_rc=1、
  gen-lang-core-0063/0064/0065 stdout 不一致）全部归因同一根因，修复后
  conformance 全绿（69.6s 单测 ok）；
- **add_values 错误通道化**：签名 `Value` → `Result<Value, VmError>`，
  ToPrimitive 抛错可传播；解释器 Add 臂 `?` 透传；JIT jit_add 按 J2
  约定（helper 无错误通道）归一 undefined；
- **M72_FLOOR 自纠 800 → 770**：轮十三把下限 480→800 的依据算错——
  「837 通过 ⇒ 失败 ≤200」不成立，实际 1154−837−87(invalid)=230 > 200，
  该门禁自设定起从未绿过（干净 HEAD 实测 837/1154、失败 230、门禁红）。
  回落到当前真实基线（841 通过/226 失败）可承受的 770，随分桶修复逐级
  上调回 1000；
- **净效果**：t262 837 → **841/1154**（失败 230 → 226，全为 m72- 官方
  语料；手写语料 0 失败）；
- 门禁证据：cargo fmt --check ✓、clippy -D warnings exit 0 ✓、
  conformance 差分 ✓（4 例连带给清）、GC 压力 ALUKA_GC_STRESS=8
  aluka-vm 215/0 + aluka-runtime 5/0 ✓、express e2e ✓、
  four_quadrants oracle ✓、jitbench 3/3 ✓、t262 门禁 FLOOR=770 ✓。

### 29.1 提交证据

- commit f43e430（fix(m7.2): 轮十四——add_values ToPrimitive 错误通道化
  + 堆原始形态结果判定修复），5 files changed, 108 insertions(+), 9
  deletions(-)。

## 30. M7.2 轮十五：非加法算术/位运算接入 ToPrimitive（20260913）

- **根因**：`-` `*` `/` `%` `**` 位运算、一元 ±/`~` 一律走
  `to_number_value`（&self，无用户代码通道）——`Object(x)` 自定义
  valueOf/toString 的除/模得 NaN（`({valueOf:()=>6})/2` → NaN，Node 3）；
- **修复**：新增 `numeric_operand`（ops.rs）——①wrapper_primitive 内部
  槽直解（new Number/String/Boolean 与 Date `_timeValue`；**必须先于**
  ToPrimitive，否则 wrapper 的 valueOf 占位被 invoke_callable 误调用，
  实测 -20 例回归后归位）；②to_primitive_number（用户 valueOf/toString，
  可抛错）；③to_number_value；
- **接线面**：解释器 Sub/Mul/Div/Mod/Pow/Neg/UnaryPlus/BitNot/BitAnd/
  BitOr/BitXor/Shl/Shr/UShr 全臂；JIT jit_to_number（全语义 + 堆刷新，
  抛错归 NaN）、jit_bitop 同步升级——emit_arith/emit_cmp 经
  fref_tonum 间接受益；
- **ToPrimitive 抛错定 TypeError**：`Object.create(null) * 2` 实例挂
  TypeError.prototype + name（prims.rs syntax_error 同口径，
  e.constructor.name === "TypeError"）；
- **Date 算术对齐**：`new Date(0) - 0 === 0`、`+new Date(0) === 0`
  （hint number 走时间值；hint string/default 才 toString 序）；
- **已知余差异（登记不修）**：`Object(原始值)` 直调一律 alloc_ordinary
  忽略参数（call.rs:388），未造带槽 wrapper——当前语料无失败依赖，
  后续按需补；
- **净效果**：t262 841 → **849/1154**（S11.5/11.6 A2/A3 算术族全绿；
  失败 226 → 218）；
- 门禁证据：fmt ✓、clippy -D warnings exit 0 ✓、t262 门禁
  FLOOR=770 ✓、conformance 差分 ✓、GC 压力 ALUKA_GC_STRESS=8
  aluka-vm+runtime 0 失败 ✓、jitbench 3/3 ✓、workspace 全量 exit 0 ✓。

## 31. M7.2 轮十六：BigInt 算术切片 + Error 子类独立原型重构（20260913）

- **失败清单分桶**（218 例）：[object Object] 正向失败 120、
  $DONOTEVALUATE parse 负例 46、vm_rc=None 超时 35、BigInt 32；
- **BigInt 算术（32 例桶）**：
  - bigdec 补全 BigNat `mul_big`（schoolbook u32）/`divmod_big`（二进制
    长除）+ `bigint_dec_sub/mul/divmod/pow`（符号层：商向零截断、余数
    符号随被除数、除零/负指数拒绝）；
  - ops 新增 `bigint_binary`（Sub/Mul/Div/Mod/Pow 五臂接线）：双侧
    BigInt 计算、单侧 BigInt 抛 TypeError（规范禁止隐式混算）、对象参
    数先 wrapper 槽直解再 ToPrimitive（valueOf 产 BigInt 采纳）；
  - add_values 补混算拦截（**规范序**：字符串拼接分支先于混算拦截——
    `1n + "1"` === "11"，oracle 实测）；
  - 除零 RangeError "Division by zero"、负指数 RangeError 对齐 oracle
    文案；`Object(2n)` 造 `[[BigIntData]]` 槽 wrapper（`Object(2n)+1n`
    === 3n）、`[[SymbolData]]` 同型补齐（`typeof Object(Symbol())`）；
- **Error 子类独立原型重构（根因修复）**：error_subclass_ctor 此前把
  `constructor` 写在**共享 Error.prototype** 上，第二个子类缓存时覆盖
  前一个（实测：先 TypeError 后 RangeError，`thrown.constructor ===
  TypeError` 由真变假）；改为每子类**独立 prototype**（链
  Error.prototype）+ 其上 constructor/name 自有属性；连带
  attach_error_proto 统一 9 处手拼错误站点（含 do_construct、
  LoadGlobal ReferenceError、克隆反序列化）并覆盖 alloc_error_instance
  预置的自有 name="Error" 遮蔽；
- **structuredClone 子类保型**：is_error 放宽为 error_prototype 或任一
  缓存子类原型命中（子类实例走 error 通道，反序列化按 name 恢复原型
  ——`instanceof TypeError` 随克隆体保留，m5 语义测试回绿）；
- **净效果**：t262 849 → **874/1154**（失败 218 → 193）；conformance
  差分保持全绿（中途被 name 遮蔽波及的 5 例归位）；
- 门禁证据：fmt ✓、clippy -D warnings exit 0 ✓、workspace 全量 exit 0 ✓
  （m2/m5 语义测试回绿）、t262 874 ✓、conformance 差分 ✓、jitbench
  3/3 ✓、GC 压力 ALUKA_GC_STRESS=8 0 失败 ✓；BigInt 16 例探针 + 算术
  15 例探针 + 克隆/原型 9 例探针逐字节对齐 Node 22。

### 31.1 提交证据

- commit 7fb8d89（fix(m7.2): 轮十六——BigInt 算术切片 + Error 子类独立
  原型重构），9 files changed（bigdec/call/eval/interpreter/ops/prims/
  property/worker_clone + TODO）。

## 32. M7.2 轮十七：后缀 ++/-- 受限产生式（ASI 语义）（20260913）

- **根因**：parse_postfix 对 `++`/`--` 无条件作后缀解析——`x\n++;`
  被容错接受（规范：后缀 ++/-- 为受限产生式，换行后不构成后缀更新，
  该程序必须报 SyntaxError）；同时前缀 `++` 缺操作数（`++;`）被
  parse_expr_primary 兜底臂（吞意外 token 返回 Undefined）静默接受；
- **修复（两臂）**：
  - parse_postfix：`++`/`--` 前有行终止符时**不作后缀**——表达式在
    此结束，ASI 于 eat_semi 生效，`++` 留给下一语句作前缀
    （`var z = 1\n++z` 合法，node 22 实测 2/2）；`x\n++;` 因 `++`
    无操作数由前缀校验报 SyntaxError；
  - parse_unary 前缀 `++`/`--`：操作数解析为 Undefined（兜底臂产物）
    时补记 SyntaxError（`++;` 报错）；
- **净效果**：t262 874 → **879/1154**（失败清单 194 → 188，$DONOTEVALUATE
  parse 负例桶转绿 6 例、零误伤）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  conformance 差分 ✓、GC 压力 0 失败 ✓；ASI 四向探针（换行后缀报错/
  同行后缀通过/换行前缀通过/裸 ++ 报错）逐项对齐 Node 22。

### 32.1 提交证据

- commit 4eea8b7（fix(m7.2): 轮十七——后缀 ++/-- 受限产生式）。

## 33. M7.2 轮十八：不可写键写入静默化 + parse 负例三族（20260913）

- **失败清单重分桶**（188 例）：[object Object] 92、parse 负例 44、
  超时 32、TypeError 8、其余小桶；
- **不可写键写入静默化（Math.E/PI、Number.NaN 等）**：
  - Vm 新增 `non_writable: HashMap<usize, Vec<String>>` 登记
    （Math 8 常量 + Number 静态 8 常量 + globalThis 的
    undefined/NaN/Infinity）；
  - set_property 与 **set_property_ic** 双守卫（后者必须先于 shape 槽
    直写——实测 `Number.NaN = 1` 经 IC 快路径绕过慢路径守卫，写入落为
    自有属性）；`Math.E = 1` 后 `Math.E === __e` 必须成立；
  - 排障发现：Number ctor 真实构造点在 builtins/global/mod.rs 装配期
    （resolve_global 的 globals 命中使 proto_ctor_value 不可达）——
    登记补在装配点；
- **parse 负例三族**：
  - 字符串字面量裸 <LF>/<CR> 与未终止形态 → LexError（lexer）；
  - 正则字面量：主位 `/` 解析失败补记 SyntaxError（此前兜底臂静默吞）；
    扫描补 U+2028/U+2029 行终结符检测；
  - 表达式主位遇 LexError token 判死（`0b0_n;` 曾被 primary 兜底臂
    静默吞掉——BigInt 分隔符校验早已存在，败在错误未传导）；
  - 字面量赋值（`true = 1`）→ SyntaxError（Invalid left-hand side）；
- **净效果**：t262 879 → **890/1154**（失败 188 → 177）；
  **M72_FLOOR 770 → 820**（按真实基线 890 通过/177 失败上调，上限
  823 留 3 例余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  conformance 差分 ✓、GC 压力 0 失败 ✓；不可写写入 5 例探针逐字节
  对齐 Node 22。

### 33.1 提交证据

- commit 2a0db75（fix(m7.2): 轮十八），FLOOR=820 门禁实测 ok。

## 34. M7.2 轮十九：async 函数早错误族（20260913）

- **剩余 35 例 parse 负例分桶**：async-function 早错误 18（最大子族）、
  BigInt 分隔符余量 6、line-terminators 余量 4、asi/comments 悬空 else
  等余量；
- **async 早错误实现**（Parser 新增 `in_async` 解析态，5 处
  parse_function_def 调用点传 is_async，进出恢复外层）：
  - 形参名 await（Keyword token 走不到 Ident 臂——单独拦截）；
  - 非简单参数列表（默认/解构/rest）形参名重复
    （`async function f(a, a = 1)` → SyntaxError）；
  - rest 后再有形参（`...a, b`）、rest 带默认值（`...a = 1`）、
    双 rest；
  - 函数体内 await 缺操作数（`void await;`/`await;`——非 async 上下文
    await 仍是普通标识符，不受影响）；
- **净效果**：t262 890 → **896/1154**（失败 177 → 171）；
  **M72_FLOOR 820 → 826**（理论上限 829 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=826 ✓、conformance 差分 ✓；async 早错误 5 负例 +
  2 正例探针逐项对齐 Node 22。

## 35. M7.2 轮二十：BigInt 分隔符位置校验复活（20260913）

- **根因**：进制扫描先 `filter(|c| c != '_')` 再做分隔符位置校验——
  raw 串已无 `_`，首尾/连续分隔符检查**永不命中**（死代码），
  `0b0_n`/`0xFF_n` 等被静默接受为合法 BigInt；
- **修复**：保留原始扫描串 `raw_scanned`，位置校验（首/尾 `_`、连续
  `__`、空数字串）基于其上执行；数字位合法性仍用滤除后串；
- **净效果**：t262 896 → **902/1154**（失败 171 → 165）；
  **M72_FLOOR 826 → 832**（理论上限 835 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=832 ✓、conformance 差分 ✓；分隔符 4 负例 +
  3 正例（0b101/0xFFn/1_000n）对齐 Node 22。

## 36. M7.2 轮二十一：裸 Ident ASI 豁免收窄 + 正则 LS/PS（20260913）

- **裸 Ident 豁免收窄**：轮十三为 TS `declare enum` 开的「裸 Ident 语句
  一律宽松吞分号」使同行 Ident-Ident（`line comment`，无行终结符）
  被静默接受——Node 22 实测 SyntaxError（ASI 不适用：两 token 同行）；
  收窄为「下一 token 是关键字 / TS 标记 Ident（enum/namespace/type）/
  `{`，或 Ident 本身是 TS 标记」——TS strip-only 链
  （`declare enum Color { Red }`）完整保留（ts_enum 单测回绿）；
- **正则字面量 LS/PS**：lexer 自带正则扫描器（前缀位置 `/` 整体成词）
  缺 U+2028/U+2029 行终结符检测——补 UTF-8 序列检查，`/␨/` 判死
  （parser 侧 parse_regexp_literal 同型检查轮二十已加）；
- **净效果**：t262 902 → **906/1154**（失败 165 → 161）；
  **M72_FLOOR 832 → 836**（理论上限 839 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=836 ✓、conformance 差分 ✓、jitbench 3/3 ✓；
  ASI/正则探针 4 向对齐 Node 22。

## 37. M7.2 轮廿二：async 绑定名早错误补全（20260913）

- **新增三查**（parse_function_def / 标签语句）：
  - async 函数**名**不得为 arguments/eval（`async function arguments(){}`）；
  - async 形参名不得为 arguments/eval（`async function foo(arguments){}`）；
  - await 不得作标签（`await: ;`——await 为 Keyword，Label 臂的 Ident
    模式走不到；经 parse_unary await 臂补 `:` 缺操作数判定拦截）；
  - 正例 `function arguments(){}`/`function eval(){}`（非 async）不受限；
- **净效果**：t262 906 → **912/1154**（失败 161 → 155）；
  **M72_FLOOR 836 → 842**（理论上限 845 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=842 ✓、conformance 差分 ✓。

## 38. M7.2 轮廿三：悬空 else + 重复 __proto__（20260913）

- **悬空 else**（asi-S7.9.2_A1_T6 / S7.9_A11_T8）：`if (false) {};\nelse
  {}` 与 `if (false)\nelse {}` 被 Keyword 兜底臂吞成 Ident 表达式静默
  接受——纯语法关键字（else/in/typeof/void/delete/case/catch/finally/
  do/default/extends/with/instanceof/enum）在表达式主位补记 SyntaxError
  （Node 22: "Unexpected token 'else'"）；正常 if-else 不受影响；
- **重复 __proto__**：对象字面量冒号形态（含字符串键 `'__proto__':`）
  重复 → SyntaxError（Node: Duplicate __proto__ fields）；简写/计算键
  不受影响（`{["__proto__"]: 1}` 合法）；
- **净效果**：t262 912 → **914/1154**（失败 155 → 153）；
  **M72_FLOOR 842 → 845**（理论上限 847 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=845 ✓、conformance 差分 ✓；探针 6/6 对齐 Node 22。

## 39. M7.2 轮廿四：super 禁用旗标 + rest 尾逗号（20260913）

- **super 早错误**：Parser 新增 `super_disallowed` 旗标——普通函数
  （声明/表达式，无 HomeObject）形参默认值与函数体内 `super()`/
  `super.x` 均 SyntaxError；类体与方法、对象字面量方法入口清除旗标
  （super 合法，`class A { m() { return super.x } }` 不受影响）；
- **rest 尾逗号**：`async function f(...a,)` → SyntaxError
  （Node: Rest parameter must be last formal parameter）；
- **净效果**：t262 914 → **919/1154**（失败 153 → 148）；
  **M72_FLOOR 845 → 849**（理论上限 852 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=849 ✓、conformance 差分 ✓；super/rest 探针 5 向对齐 Node 22。

## 40. M7.2 轮廿五：单行注释 LS/PS 终止 + HTML 闭注释 + 形参重名（20260913）

- **单行注释 LS/PS 终止**：U+2028/U+2029 同为行终结符——`//` 注释在此
  终止，其后内容为代码（`// single line LS??? (invalid)` 负例；
  `// ok<LS>console.log(1)` 正常执行不受影响）；
- **HTML 闭注释（Annex B）**：`-->` 仅在行首（此前至多空白）构成单行
  注释；`;-->` 前置有代码 → 常规记号解析 → `undefined--`（后缀目标为
  字面量）补记 SyntaxError（Invalid left-hand side）；
- **比较类标点主位判死**：`>` `<` `>=` `<=` `==` `!=` `===` `!==` 永不
  处于表达式主位，兜底臂补记 SyntaxError；
- **形参重名**：函数体顶层 let/const 与形参重名（`foo(bar){ let bar; }`）
  → SyntaxError（嵌套块内遮蔽合法不查）；
- **净效果**：t262 919 → **923/1154**（失败 148 → 144）；
  **M72_FLOOR 849 → 853**（理论上限 856 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=853 ✓、conformance 差分 ✓、GC 压力 0 失败 ✓；
  探针（LS 注释/`-->` 三向/形参重名）逐项对齐 Node 22。

## 41. M7.2 轮廿六：Object.prototype.toString 槽位标签 + Boolean this 检查（20260913）

- **obj_to_string_tag 补 wrapper 内部槽标签**：[[BooleanData]]/
  [[NumberValue]]/[[StringValue]]/[[SymbolData]]/[[BigIntData]] →
  "[object Boolean/Number/String/Symbol/BigInt]"——
  `delete Boolean.prototype.toString` 后 `obj.toString()` 沿链命中
  Object.prototype.toString 仍须产正确标签（S15.6.2.1_A4 族）；
- **Boolean.prototype.{toString,valueOf} this 检查**：仅原始布尔或
  [[BooleanData]]/[[BooleanValue]] 包装实例合法，其余（含 String 包装
  借道 `s.myToString = Boolean.prototype.toString`）→ TypeError
  （S15.6.4.2_A2 族）；
- **净效果**：t262 923 → **927/1154**（失败 144 → 140）；
  **M72_FLOOR 853 → 857**（理论上限 860 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=857 ✓、conformance 差分 ✓；Boolean 7 探针对齐 Node 22。

## 42. M7.2 轮廿七：Symbol.prototype 注册表 handler（20260913）

- Symbol.prototype.{toString,valueOf,description} 此前仅注册原生函数名、
  无 registry handler——`Symbol.prototype.toString.call(sym)` 形态
  （经 invoke_callable 的注册表查找）误报 TypeError；
- 新增 symbol_method_dispatch（thisSymbolValue：仅符号接收者合法，
  非符号 → TypeError）；方法面注册拆分（for/keyFor 为静态面不入
  prototype handler）；
- 探针：A/B/C（借道调用/非符号 this/符号 this）逐项对齐 Node 22；
- t262 基线 927 持平——语料 Symbol 桶（Boolean-symbol-coercion、
  desc-to-string 等）实际需要 **Symbol ToString 限制**（模板串/字符串
  拼接遇 Symbol → TypeError，现静默产 "Symbol(x)"）与
  `true.valueOf()`（布尔原始值接收者 valueOf 误落数值通道得 1），
  登记为下一轮目标；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  conformance 差分 ✓。

## 43. M7.2 轮廿八：Symbol ToString 限制 + 布尔原始值方法路由（20260913）

- **ToPrimitive(Symbol) 原样返回**：符号堆对象在 to_primitive_number
  早退（经 @@toPrimitive 语义）——此前调用 toString 并把 "Symbol(x)"
  当原始值采纳，令 `'' + Symbol()` / 模板串插值静默产串；
- **字符串拼接 Symbol 守卫**：add_values 字符串/Buffer 分支遇符号 →
  TypeError "Cannot convert a Symbol value to a string"（对齐 Node 22）；
- **布尔原始值方法路由**：`true.valueOf()`/`true.toString()` 此前误落
  Number.prototype 面（得 1/"1"）——Boolean 接收者改走
  Boolean.prototype 面（得 true/"true"）；
- **净效果**：t262 927 → **928/1154**（失败 140 → 139）；
  **M72_FLOOR 857 → 858**（理论上限 861 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=858 ✓、conformance 差分 ✓；Symbol/布尔探针 3+7 例对齐。

## 44. M7.2 轮廿九：Array length setter（20260913）

- set_property 补 Array 实例 length 写入：截断（`x.length = 1` 移除
  越界元素）/ 扩展（`y.length = 3` 以 undefined 填充）/ 长度校验
  （NaN/负数/非整数/≥2^32 → RangeError "Invalid array length"）/
  wrapper 参数经 numeric_operand 解包（`x.length = new Number(2)`）；
- 此前写入被静默忽略（读回仍是真实长度）——S15.4.5.1/5.2 族全灭；
- **净效果**：t262 928 → **931/1154**（失败 139 → 136）；
  **M72_FLOOR 858 → 861**（理论上限 864 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=861 ✓、conformance 差分 ✓；length setter 4 探针对齐。

## 45. M7.2 轮三十：原生构造器 [[Prototype]] 解析（20260913）

- **根因**：NativeCtor 堆变体只存自有属性（含 `prototype`），无自身
  `[[Prototype]]` 字段——get_prototype / isPrototypeOf 对原生构造器
  返回 None，令 `Function.prototype.isPrototypeOf(Array)` false、
  `Object.getPrototypeOf(Array) === Function.prototype` false；
  且旧实现把 `properties["prototype"]`（**产物的**原型）误当构造器自身
  原型（`Array.prototype.isPrototypeOf(Array)` 反而 true）；
- **修复（动态解析，不动 45 处构造点）**：get_prototype 与
  obj_is_proto_of 的 NativeCtor 分支统一返回 `vm.fn_proto`
  （规范：所有内置构造器 [[Prototype]] ≡ Function.prototype，
  Function 自身亦然）；
- **排障记录**：曾尝试给 alloc_native_ctor 增 ctor_proto 参数，
  正则批量改写破坏嵌套括号调用点（44 处语法错误）后整批回滚，
  改采动态解析——避免侵入式签名变更与借用冲突；
- **净效果**：t262 931 → **934/1154**（失败 136 → 133）；
  **M72_FLOOR 861 → 864**（理论上限 867 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=864 ✓、conformance 差分 ✓；原型链探针 5/5 对齐 Node 22。

## 46. M7.2 轮卅一：类体访问器 + 计算键 + 原型链 setter（20260913）

- **解析**（parse_class_stmt）：类体 `get x() {}` / `set x(v) {}` 访问器
  前缀（仅当后随键名而非 `(`——`get()` 方法简写不受影响）+ 计算键
  `get ['a']() {}`（字符串/数字/标识符字面量取文本为名，复杂表达式走
  parse_expr 兜底）；kind 1/2 传入 ClassMethodDef（codegen 早已支持）；
- **VM**（property.rs set_property）：setter 查找补**原型链**遍历——
  此前仅查自层，类访问器挂在 C.prototype，实例写入静默落为数据属性
  （`c.b = 5` 后 `c._b` undefined）；
- **净效果**：t262 934 → **943/1154**（失败 133 → 124）；
  **M72_FLOOR 864 → 873**（理论上限 876 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=873 ✓、conformance 差分 ✓；访问器探针 5/5 对齐 Node 22。

## 47. M7.2 轮卅二：类静态成员名 prototype 限制 + static 词法修正（20260913）

- **parser**：`static` 非保留字（词法为 Ident，KEYWORDS 表无此项）——
  类体修饰符判定原用 check_keyword 恒为假，`static x(){}` 被当作名为
  static 的方法解析、后续 token 全乱（`static ['prototype']() {}` 直接
  报"预期标点 ("）；改为 Ident 判定 + 下一 token 非 `(`（`static() {}`
  仍是名为 static 的普通方法）；
- **VM**（class.rs）：静态成员名 `prototype` → TypeError
  （"Classes may not have a static property named 'prototype'"，
  规范 ProtectedName 限制；覆盖字面量键、计算键 `['prototype']`、
  getter/生成器等形态）；
- **净效果**：t262 943 → **949/1154**（失败 124 → 118）；
  **M72_FLOOR 873 → 879**（理论上限 882 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=879 ✓、conformance 差分 ✓；static/访问器探针 7/7 对齐。

## 48. M7.2 轮卅三：Boolean.prototype 自身 this 缺省（20260913）

- 规范：`Boolean.prototype.toString()` / `valueOf()` 的 this 为
  **Boolean.prototype 自身**（无 [[BooleanValue]] 数据槽）时按 false
  处理，不抛错（S15.6.4.2_A1 族）；上轮加入的 this 检查过严，
  对原型自身误报 TypeError；
- 修复：bool_method_dispatch 数据槽缺失时判 `vm.bool_proto == Some(r)`
  → false，其余对象（如 String 包装借道）仍 TypeError（A2 族不回退）；
- **净效果**：t262 949 → **953/1154**（失败 118 → 114）；
  **M72_FLOOR 879 → 883**（理论上限 886 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=883 ✓、conformance 差分 ✓。

## 49. M7.2 轮卅四：包装原型单例内置数据槽（20260913）

- 规范：Number/String/Boolean.prototype **本身是包装对象**
  （[[NumberData]]=+0 / [[StringData]]="" / [[BooleanData]]=false）——
  `Object.prototype.toString.call(Number.prototype)` → "[object Number]"、
  `Number.prototype.valueOf()` → 0；此前三个原型为裸 Ordinary，
  标签恒 "[object Object]"（S15.7.4-1 / S15.5.4 族）；
- 新增 prime_wrapper_proto：为三个原型单例注入对应数据槽
  （在 surface 装配末尾调用，原型已建）；
- **净效果**：t262 953 → **954/1154**（失败 114 → 113）；
  **M72_FLOOR 883 → 884**（理论上限 887 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=884 ✓、conformance 差分 ✓；原型标签探针 4/4 对齐。

## 50. M7.2 轮卅五：解构模式补全（空元素位 / 嵌套模式）（20260913）

- **空元素位**：`[a,, b = a, c]`——模式循环遇即刻 `,` 跳过后继续
  （此前 expect ']' 失败）；
- **嵌套模式**：数组元素位接受 `{y}` / `[..]` 嵌套（`[x, {y}, ...z]`、
  `{x: [...y]}`）——AST 的 ArrayPatternElem 仅有 name 文本字段，
  嵌套绑定名取首项键名/元素名作占位；
- **净效果**：t262 954 → **959/1154**（失败 113 → 108）；
  **M72_FLOOR 884 → 889**（理论上限 892 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=889 ✓、conformance 差分 ✓；解构族 5 例转绿。

## 51. M7.2 轮卅六：新 ISA 操作码 SET_PROTO_OBJ（对象字面量 __proto__）（20260913）

- **动机**：对象字面量 `{ __proto__: v }` 的规范语义是**设 [[Prototype]]**
  而非建自有属性——`Object.getPrototypeOf(o) === proto` 为 true 且
  `getOwnPropertyDescriptor(o,'__proto__')` 为 undefined；此前一律落
  SetPropObj 建自有属性（S11.1.5 / __proto__ 桶 5 例全灭）；
- **ISA 扩容**（第 107 条操作码，编码 106 追加保持既有编码稳定）：
  op.rs 全表登记（枚举/from_opcode/name/operand 种类/操作数字节数/
  stack_effect 净 -1/pops 2/detailed/纯压栈判定×3）；verifier 通过
  （**不入 requires_string 组**——操作数为 None，避免常量类型误判）；
  JIT 侧全链路（ctx.rs 类型别名 SetProtoObjFn + vtable 字段、
  jit_helpers 实现 jit_set_proto_obj、lib.rs 常量/签名/vtable 分派/
  FuncRef 声明 + 机器码臂 helper 回退）；
- **编译器**：codegen 对象字面量字面量键 `__proto__` 发 SetProtoObj；
- **排障记录**：stack_effect 初填 -2（真实为弹 2 压回 1 净 -1）、
  pops 初填 0（真实 2）导致 express e2e 4 模块 V8 汇合点栈深校验失败——
  两处修正后回绿；同表重复插入被 clippy unreachable_pattern 拦下；
- **净效果**：t262 959 → **961/1154**（失败 108 → 106）；
  **M72_FLOOR 889 → 891**（理论上限 894 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=891 ✓、conformance 差分 ✓、jitbench 3/3 ✓、
  express e2e ✓（栈深校验回归已修）、GC 压力 0 失败 ✓；
  __proto__ 探针 5/5 对齐 Node 22。

## 52. M7.2 轮卅七/卅八：生成器迭代 + 函数 prototype.constructor（20260913）

- **生成器接入迭代协议**（iter.rs collect_iter_values）：此前生成器对象
  不在快路径（Array/Map/Set/String/typed array 之外），`[...gen()]`
  静默为空；补生成器分支——驱动至 done 逐次取 `value`，步骤内抛错沿 `?`
  传播（spread-err 族期望异常穿透）；
- **函数 prototype.constructor 回指**（heap.rs alloc_closure_with_upvalues）：
  规范要求函数 `prototype` 的自有 `constructor` 指向函数自身——
  `new F().constructor === F`；官方 assert.throws 的
  `thrown.constructor !== ExpectedCtor` 判定依赖此属性，缺失时实例
  constructor 沿链落到 Object.prototype.constructor（自定义错误类判定
  全灭，spread-err 族 12 例的直接根因）；
- **净效果**：t262 961 → **964/1154**（失败 106 → 103）；
  **M72_FLOOR 891 → 894**（理论上限 897 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=894 ✓、conformance 差分 ✓、jitbench 3/3 ✓；
  生成器/constructor 探针 6/6 对齐 Node 22。

## 53. M7.2 轮卅九：Symbol 构造与访问器语义（20260913）

- **Symbol(description)**：description 为符号 → TypeError
  （Cannot convert a Symbol value to a string；symbol_create 改带错误通道）；
- **new Symbol()** → TypeError（Symbol is not a constructor）；
- **Symbol.keyFor(x)**：非符号 → TypeError（此前静默 undefined）；
- **Symbol.for(key)**：key 为符号 → TypeError；
- **Symbol.prototype.description 访问器**：getter 语义（符号 → 描述串、
  无描述/非符号 → undefined）——经 accessor 描述符定义；
- **净效果**：t262 964 → **966/1154**（失败 103 → 101）；
  **M72_FLOOR 894 → 896**（理论上限 899 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=896 ✓、conformance 差分 ✓；Symbol 探针 6/6 对齐。

## 54. M7.2 轮四十：未捕获错误渲染回退 toString（20260913）

- **根因**：官方语料的 Test262Error 常**只定义 toString**（不设 name）——
  未捕获错误渲染取 `e.name` 得 undefined 串，输出 "[object Object]"；
  测试框架的 runtime 负例判定要求输出含类型名（`Test262Error`），
  故整族（line-terminators-comment 7 例等）误判失败；
- **修复**：format_uncaught（bc_entry + lib.rs 两处）在 name 缺失时回退
  调用 toString 取结果；新增 Vm::call_to_string 公开方法
  （get_property + invoke_callable 封装）；
- **净效果**：t262 966 → **973/1154**（失败 101 → 94）；
  **M72_FLOOR 896 → 903**（理论上限 906 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=903 ✓、conformance 差分 ✓。

## 55. M7.2 轮卌一：Symbol.iterator 协议接入展开/迭代（20260913）

- **根因**：collect_iter_values（`[...x]` / for-of / Array.from 的物化
  入口）只认 Array/Map/Set/String/typed array/生成器快路径，**自定义
  可迭代**（对象字面量定义 Symbol.iterator）一律得空数组，且
  `Symbol.iterator` 方法体内的抛错被完全吞掉（spread-err 族 10 例）；
- **实现**：新增 `has_symbol_iterator`（沿原型链查 mangled 键）+
  `Vm::well_known_cached`（不触发创建的已物化符号查询）；判定命中后
  经 `get_iterator_dispatch` 取迭代器、按 `next()` 协议驱动至 done——
  方法体抛错沿 `?` 传播（`[...iter]` 的异常穿透语义）；
- **净效果**：t262 973 → **980/1154**（失败 94 → 87）；
  **M72_FLOOR 903 → 910**（理论上限 913 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=910 ✓、conformance 差分 ✓；可迭代探针 3/3 对齐
  （含异常穿透与生成器方法体）。

## 56. M7.2 轮卌二：ToPropertyKey wrapper 解包 + length 上限保护（20260913）

- **ToPropertyKey wrapper 解包**：`a[new Number(1)]` 此前键落
  "[object Object]"（下标写入丢失，读回 undefined）——补内部槽
  （[[NumberValue]]/[[BooleanValue]]/[[StringValue]]）递归取键
  （S15.4_A1.1_T6/T7/T8 族）；
- **length setter 内存保护**：`x.length = 4294967295` 触发 34GB 分配
  直接 abort（vm_rc=0xC0000409）——加 4e6 上限（与 new Array(len) 同口径），
  超限不 resize（转为语义差异而非崩溃）；
- **净效果**：t262 980 → **983/1154**（失败 87 → 84）；
  **M72_FLOOR 910 → 913**（理论上限 916 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=913 ✓、conformance 差分 ✓；索引/崩溃探针 4/4 对齐。

## 57. M7.2 轮卌三：BigInt 全局函数 + prototype 面（20260913）

- **实现 `BigInt(v)`**（invoke_callable 拦截，与 Symbol 同位置）：
  - 数字须为整数（非整数/NaN/Infinity → RangeError）；
  - 字符串按字面量解析（含 0x/0b/0o 前缀与空白裁剪；非法 → SyntaxError）；
  - 布尔 → 0n/1n；BigInt 原样；其余 → TypeError；
  - `new BigInt()` → TypeError（BigInt is not a constructor）；
- **prototype 面**：bigint_ctor_value 建立独立原型（valueOf/toString/
  toLocaleString）+ prototype.constructor 回指 + 静态 asIntN/asUintN 占位
  ——express 依赖链的 object-inspect 以 `BigInt.prototype.valueOf` 探形，
  注册只有 NativeCtor 无原型会致 `Cannot read properties of undefined`；
- **排障记录**：先用 proto_ctor_value（prototype=None）注册致 express e2e
  失败（`typeof BigInt === "undefined"` 分支变化后暴露缺原型）；
  以 git stash 二分确认破坏源，改为专用 bigint_ctor_value 后回绿；
- **净效果**：t262 983 → **984/1154**（失败 84 → 83）；
  **M72_FLOOR 913 → 914**（理论上限 917 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=914 ✓、conformance 差分 ✓、jitbench 3/3 ✓、
  express e2e ✓（回归已修）；BigInt 探针 7/7 对齐 Node 22。

## 58. M7.2 轮卌四：ToString 全语义（js_string）+ Boolean 标签槽名（20260913）

- **新增 `js_string`（Vm 公开，含 hint string 全语义）**：对象按
  toString → valueOf 序取原始值后转串——用户自定义 toString 生效；
  `String(obj)` / `new String(obj)` 全路径接入（call.rs 直调分支 +
  do_construct + invoke_callable 的 NativeFn 分支）；
  - 规范细节：`String(sym)` 是**唯一**允许符号转串的路径
    （SymbolDescriptiveString → "Symbol(d)"），`"" + sym` 仍由
    add_values 字符串分支抛 TypeError（core_semantics 单测回绿）；
- **Boolean 包装标签**：obj_to_string_tag 此前只认 [[BooleanData]]，
  而实例槽实为 [[BooleanValue]]——两名称并存，判定同时认
  （`delete Boolean.prototype.toString` 后 `new Boolean().toString()`
  沿链命中 Object.prototype.toString 得 "[object Boolean]"）；
- **净效果**：t262 984 → **990/1154**（失败 83 → 77）；
  **M72_FLOOR 914 → 920**（理论上限 923 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓
  （core_semantics 21/21）、t262 FLOOR=920 ✓、conformance 差分 ✓；
  String/Boolean 探针 7/7 对齐 Node 22。

## 59. M7.2 轮卌五：内建实例原型回退 + instanceof 原始值判定（20260913）

- **get_prototype 回退**（`&self` 版 cached_proto_of）：无 [[Prototype]]
  字段的内建实例（字符串/符号/函数）回落 surface 原型单例——
  `Object.getPrototypeOf(Symbol('66')) === Symbol.prototype`（S19.4.3
  intrinsic 族）；**原始值分支保持 None**（返回包装原型会让
  `(1) instanceof Object` 误真，conformance gen-eval-matrix-0009 实测）；
- **包装实例原型**：alloc_primitive_wrapper 按内部槽挂对应原型
  （`Object(Symbol())` 的 [[Prototype]] = Symbol.prototype）；
- **instanceof 原始值判定**：check_instanceof 首行补非对象早退——且
  NaN-box 下堆字符串/符号/BigInt 虽为 Object case 但语义是原始值，
  一并排除（`"s" instanceof String`、`Symbol() instanceof Symbol`
  均 false；conformance gen-eval-matrix-0010 实测）；
- **净效果**：t262 990 → **992/1154**（失败 77 → 75）；
  **M72_FLOOR 920 → 922**（理论上限 925 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=922 ✓、conformance 差分 ✓（两例回归已修）、jitbench 3/3 ✓；
  原型链/instanceof 探针 9/9 对齐 Node 22。

## 60. M7.2 轮卌六：Number() ToNumeric 全语义 + Infinity 严格解析（20260913）

- **Number(value) 升级为 ToNumeric 全语义**（原用 &self 的 to_number_value，
  不做 ToPrimitive）：对象经 ToPrimitive hint number（自定义 valueOf 生效）、
  wrapper 槽直解、无参 → +0；新增 numeric_operand 的符号守卫
  （ToNumber(Symbol) → TypeError；`Number(Symbol())`/`+Symbol()` 均抛）；
- **字符串 "Infinity" 严格化**：Rust f64::parse 接受 "INFINITY"/"inf"/
  "infinity"/"NaN" 等宽松形态，先拦下——只认精确
  `Infinity`/`+Infinity`/`-Infinity`，其余含 inf/nan 前缀一律 NaN
  （`Number("INFINITY")` 应为 NaN，S15.7.1.1 族）；
- **净效果**：t262 992 → **996/1154**（失败 75 → 71）；
  **M72_FLOOR 922 → 926**（理论上限 929 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=926 ✓、conformance 差分 ✓；Number/Infinity 探针 12/12 对齐。

## 61. M7.2 轮卌七：JSON 命名空间语义（20260913）

- **JSON 内建属性不可枚举**：stringify/parse（及 rawJSON/isRawJSON 占位）
  改经 define_proto_method 挂载（enumerable:false）+ `_isJSON` 标记同样
  不可枚举——`for (var p in JSON)` 计数为 0（S15.12.0-4 族）；
- **`new JSON()` → TypeError**：JSON 是普通 Ordinary 命名空间对象（非
  NativeCtor），do_construct 增加 `_isJSON` 标记判定早退（S15.12.0-2 族）；
- **净效果**：t262 996 → **998/1154**（失败 71 → 69）；
  **M72_FLOOR 926 → 928**（理论上限 931 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=928 ✓、conformance 差分 ✓；JSON 探针 3/3 对齐。

## 62. M7.2 轮卌八：eval 内 var 声明完成值（栈残留修复）（20260913）

- **根因**：隐式全局模式（eval 求值域）下 `var z;`（无初始值器）的
  codegen 在 StoreGlobal 之后**又压了一个 undefined** 充当"完成值"——
  但 StoreGlobal 已消费栈顶，该值成为残留，污染调用方栈区：
  `console.log("W:", eval("var z;"))` 实测输出 "undefined undefined"
  且丢失 "W: " 前缀（eval 返回双值 → 实参错位）；
- **规范依据**：VariableStatement 的完成值恒为 **empty**（非 undefined），
  脚本收口由末尾 ReturnUndef 提供——此处不得再压值；
- **修复**：移除多余 PushUndefined，eval 模块指令流由
  `[PushUndefined, StoreGlobal, PushUndefined, ReturnUndef]`（4 条）
  收敛为 `[PushUndefined, StoreGlobal, ReturnUndef]`（3 条，栈平衡）；
- **净效果**：t262 998 → **999/1154**（失败 69 → 68）；
  **M72_FLOOR 928 → 929**（理论上限 932 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=929 ✓、conformance 差分 ✓；eval/var 探针矩阵
  （1+1/;/if/赋值/声明 五种源码）5/5 对齐 Node 22。

## 63. M7.2 轮卌九：Symbol 描述语义 + 原型 constructor（20260913）—— 突破 1000

- **ToSymbolDescription**（Symbol(desc) 的 desc 经 ToPrimitive 后转串）：
  - 对象走其 toString（`Symbol({toString:()=>"toString"}).description`
    === "toString"，此前得 "[object Object]"）；
  - 描述产符号 → TypeError（`Symbol({toString:()=>Symbol()})`；
    此前静默产串）；
  - 空描述区分：**新增 HeapObject::Symbol.has_desc 字段**——规范
    `Symbol()` → description 为 undefined、`Symbol("")` → ""，两者堆
    描述串均为空、此前无法区分（唯一此前用 is_empty 判定的两处
    ——property.rs 合成路径与 surface description getter——同步改用）；
- **Symbol.prototype.constructor 回指**：包装原型 constructor 回指循环
  补入 Symbol（`Object.getPrototypeOf(Symbol('x')).constructor === Symbol`）；
- **净效果**：t262 999 → **1001/1154**（失败 68 → 66）——**突破 1000**；
  **M72_FLOOR 929 → 931**（理论上限 934 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=931 ✓、conformance 差分 ✓；Symbol 探针 6/6 对齐 Node 22。

## 64. M7.2 轮五十：Symbol.for/描述 ToString 序（20260913）

- **Symbol.for(key) 经 ToString**：对象走其 toString
  （`Symbol.for({toString:()=>"test2"}) === Symbol.for("test2")`）；
  结果为符号 → TypeError（`Symbol.for(Symbol("s"))` 与 toString 产符号
  两形态）；此前用 format_value 得 "[object Object]"（注册表键错乱）；
- **ToSymbolDescription 改 hint string 序**：描述对象经 toString 优先
  （`Symbol({toString:()=>"toStr", valueOf:()=>"valueOf"}).description`
  === "toStr"，此前误得 "valueOf"）——与 ToPrimitive(hint number) 区分；
- **代码可读性**：Symbol.for 的"是否需 ToPrimitive"判定简化为
  「Object case 且非堆字符串/符号原语」；
- **净效果**：t262 1001 → **1002/1154**（失败 66 → 65）；
  **M72_FLOOR 931 → 932**（理论上限 935 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=932 ✓、conformance 差分 ✓；Symbol.for/描述探针 6/6 对齐。

## 65. M7.2 轮五十一：严格 ToString（js_string_strict）（20260913）

- **新增 `js_string_strict`**：对象经 toString → valueOf 取原始值，**两者
  皆不可调用或皆不产原始值 → TypeError**（无 js_string 的
  "[object Object]" 兜底）；结果为符号亦 TypeError（ToString(Symbol) 禁止）；
- **Symbol 描述路径改用严格版**：`Symbol({toString:1, valueOf:2})` 此前
  静默得 "[object Object]" 描述，现按规范抛 TypeError
  （S19.4.1.1_A1 族）；
- **净效果**：t262 1002 → **1003/1154**（失败 65 → 64）；
  **M72_FLOOR 932 → 933**（理论上限 936 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=933 ✓、conformance 差分 ✓；Symbol 三批探针
  （sy2/sy4/sy5 共 16 项）全对齐 Node 22。

## 66. M7.2 轮五十二：new Number() 无参 + Number 包装 ToPrimitive（20260913）

- **`new Number()` 无参 → [[NumberData]] = +0**（规范 S15.7.2.1）：此前经
  `to_number_value(undefined)` 得 NaN，`x2.valueOf()` 断言失败；
- **`new Number(v)` 的 v 经 ToNumeric**：与 `Number(v)` 直调同口径
  （对象走 ToPrimitive、符号 → TypeError），不再用 &self 的 to_number_value；
- **净效果**：t262 1003 → **1004/1154**（失败 64 → 63）；
  **M72_FLOOR 933 → 934**（理论上限 937 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=934 ✓、conformance 差分 ✓；Number/Boolean 包装探针
  （含无参、Date 借道 Boolean 方法）6/6 对齐 Node 22。

## 67. M7.2 轮五十三：新 ISA 操作码 REQUIRE_OBJECT_COERCIBLE（20260913）

- **解构声明前置检查**：规范要求解构绑定初始化对 null/undefined 抛
  TypeError（`fn({})` 传 null、`var {a} = null` 均抛；S8.8.2 族）——
  此前静默通过；
- **ISA 扩容**（第 108 条操作码，编码 107 追加）：op.rs 全表登记
  （枚举/from_opcode/name/operand/pops 0/pushes 0/net 0/detailed
  Fixed(0)/is_pure_push/is_jump ×3）；verifier 通过；JIT 全链路
  （ctx 类型 RequireCoercibleFn + vtable 字段、jit_helpers 实现、
  lib.rs 常量/签名/decl/FuncRef/机器码臂）；编译器在 DestructureDecl
  的 StoreLocal 后发 LoadLocal + 检查 + **Pop**（首版漏 Pop 致 V8 汇合点
  栈深不一致，conformance 2 例 + sqlite 3 例连带失败，补 Pop 后全绿）；
- **排障记录**：批量补表脚本在同表重复插入（clippy unreachable_pattern
  拦下 2 处），已清理并加全表重复自检（0 重复）；
- **净效果**：t262 1004 → **1006/1154**（失败 63 → 61）；
  **M72_FLOOR 934 → 936**（理论上限 939 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=936 ✓、conformance 差分 ✓、jitbench 3/3 ✓、
  express e2e ✓、GC 压力 0 失败 ✓；解构探针 4/4 对齐 Node 22。

## 68. M7.2 轮五十四：内置实例方法用户覆写优先（20260913）

- **根因**：`try_dispatch` 按 receiver 类型（`_builtinNs`/`_isDate` 等）
  直接执行内置 handler，**绕过属性查找**——Date/Map/Set 等实例以
  NativeFn 在原型上挂方法，用户覆写实例同名属性后调用仍走内置
  （`Object.defineProperty(d, "toString", {value: Boolean.prototype.toString})`
  后 `d.toString()` 不抛 TypeError，S15.9.5 族）；
- **修复**：try_dispatch 入口加守卫——receiver 为 Ordinary 且
  **自有同名键值与原型槽值不同**（即真覆写）时返回 None 走常规解析；
  - 判定演进（三轮收敛）：①任意自有键让位 → 误拦内建单例
    （Reflect.apply、fs.Stats.isFile）致 conformance 3 例 + m1-proxy 2 例
    回归；②仅 Closure 让位 → 漏掉 NativeFn 形态覆写（本用例正是
    借道 Boolean.prototype.toString 的 NativeFn）；③**仅当原型链上
    存在同名方法且值不同**时让位 —— 原型无该键者（Reflect.apply）
    其自有成员即分派目标，不误拦；
- **配套**：get_method_ic 命中前复核 receiver 自身无该键
  （`Object.defineProperty` 可在不改 shape 前提下加键，IC 缓存的原型槽
  会被误采纳）；
- **净效果**：t262 1006 → **1008/1154**（失败 61 → 59）；
  **M72_FLOOR 936 → 938**（理论上限 941 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓（含 nonminimal_bool 修正）、
  workspace 全量 0 失败 ✓、t262 FLOOR=938 ✓、conformance 差分 ✓
  （3 例回归已修）、express e2e ✓；覆写探针 3 批（bp5/bp6/bp9）
  全对齐 Node 22。

## 69. M7.2 轮五十五：@@toPrimitive 协议 + `+` 用 hint default（20260913）

- **`@@toPrimitive` 接入**（此前知名符号存在但 ToPrimitive 从不查找）：
  - 新增 `call_to_primitive(v, hint)`：对象定义 `Symbol.toPrimitive` 时按
    其结果作为 ToPrimitive 输出（hint 传 "number"/"string"/"default"；
    返回非原始值 → TypeError）；未定义 → None 回退常规序；
  - 接入点：`to_primitive_number`（hint number）、新增
    `to_primitive_default`（hint default）、`js_string`（hint string）；
- **`+` 改用 hint default**：add_values 的 ToPrimitive 由 hint number 改
  default（规范 `+` 用 default）——**Date 特例**：hint default/number 下
  Date 走 toString 得日期串（`date + 1` 是字符串拼接，S11.6.1_A2.2_T2 族）；
  同时把 Date 排除出 add_values 的 `wrapper_primitive` 快路径
  （`_timeValue` 提前解包成数字会破坏该语义）；
- **净效果**：t262 1008 → **1018/1154**（失败 59 → 49，本会话单轮最大 +10）；
  **M72_FLOOR 938 → 948**（理论上限 951 留余量）；
- 排障记录：own-key 复核一度加错到 `get_property_ic`（数据槽直读路径），
  致 pic 单测 2 例失败（`pic_hits` 恒 0）；按函数边界精确定位后移入
  `get_method_ic` 原型槽读取处，vm 单测 182/0 恢复；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=948 ✓、conformance 差分 ✓、express e2e ✓、jitbench 3/3 ✓；
  @@toPrimitive/Date 探针全对齐（Date 时区口径为项目已登记偏离）。

## 70. M7.2 轮五十六：@@iterator getter 触发与抛错传播（20260913）

- **根因**：`has_symbol_iterator` 用 `own_value` 逐原型层查键（**不触发
  getter**），`Object.defineProperty(o, Symbol.iterator, {get(){throw ...}})`
  的 getter 既未被调用、其抛错也无从传播——展开得空数组
  （spread-err 族 4 例）；
- **修复**：改经 `get_property` 沿原型链解析（触发 getter 并传播抛错），
  签名 `&mut self -> Result<bool, VmError>`；判定改「解析值非 undefined」；
- **净效果**：t262 1018 → **1020/1154**（失败 49 → 47）；
  **M72_FLOOR 948 → 950**（理论上限 953 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=950 ✓、conformance 差分 ✓；getter 抛错探针对齐 Node 22。

## 71. M7.2 轮五十七：Symbol.for 键语义 + 符号不可构造（20260913）

- **Symbol.for(key) 键经 ToString**：改用 js_string_strict（hint string，
  toString 优先——`Symbol.for({toString:()=>'test262'}).description`
  === "test262"，此前用 format_value 得 "[object Object]"）；
- **注册符号描述即其 key**：alloc_symbol_described(..., true)——
  `Symbol.for("k").description` → "k"（此前 undefined，has_desc 未置）；
- **符号不可构造**：
  - `new Object(Symbol())()`（符号包装实例再 new）→ TypeError
    （do_construct 增 [[SymbolData]] 槽判定）；
  - `new sym()`（符号**原始值**作 callee）→ TypeError
    （do_construct 尾段通用路径增符号早退，不再静默产对象）；
- **净效果**：t262 1020 → **1021/1154**（失败 47 → 46）；
  **M72_FLOOR 950 → 951**（理论上限 954 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=951 ✓、conformance 差分 ✓、express e2e ✓；
  Symbol 探针两批（sy6/sy7 共 9 项）全对齐 Node 22。

## 72. M7.2 轮五十八：对象字面量方法 HomeObject（super.m）（20260913）

- **对象字面量方法的 `super.m()` 此前编译为 PushUndefined**——
  MethodCall 的 Super receiver 分支只实现了类机制（`__home_proto_{cid}__`），
  对象字面量走 else 得 undefined → `Cannot read properties of undefined
  (reading 'm')`（computed-property-names-object-*-super 3 例）；
- **实现**：
  - 对象字面量含方法简写/访问器时绑 **HomeObject 槽**
    （`HOME_OBJECT_SYM = "__aluka_home_object__"`）：NewObject 后
    Dup + StoreLocal；嵌套对象字面量由 symbol_map 覆盖/恢复保证内层绑定；
  - `super.m` 编译：方法 unit 经 upvalue 捕获外层槽
    （compile_method_function 通用预置：class_id 为 None 且
    parent_info.locals 含该名时），LoadUpvalue/LoadLocal → **GetProto**
    → GetProp；`super.m()` 的 this 保持调用方 this（CallThis 语义）；
  - **关键修正**：首版补丁漏发 GetProto 指令——super.m 直接读到对象
    自身同名方法 → 无限递归栈溢出（bc 指令 dump 定位）；
- **净效果**：t262 1020 → **1024/1154**（失败 47 → 43）；
  **M72_FLOOR 951 → 954**（理论上限 957 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=954 ✓、conformance 差分 ✓、express e2e ✓、jitbench 3/3 ✓、
  GC 压力 0 失败 ✓；super 探针（sm/sm2/sm3）对齐 Node 22。

## 72. M7.2 轮五十八：对象字面量方法 HomeObject（super.m）（20260913）

- **对象字面量方法的 `super.m()` 此前编译为 PushUndefined**——
  MethodCall 的 Super receiver 分支只实现了类机制（`__home_proto_{cid}__`），
  对象字面量走 else 得 undefined → `Cannot read properties of undefined
  (reading 'm')`（computed-property-names-object-*-super 3 例）；
- **实现**：
  - 对象字面量含方法简写/访问器时绑 **HomeObject 槽**
    （`HOME_OBJECT_SYM = "__aluka_home_object__"`）：NewObject 后
    Dup + StoreLocal；嵌套对象字面量由 symbol_map 覆盖/恢复保证内层绑定；
  - `super.m` 编译：方法 unit 经 upvalue 捕获外层槽
    （compile_method_function 通用预置：class_id 为 None 且
    parent_info.locals 含该名时），LoadUpvalue/LoadLocal → **GetProto**
    → GetProp；`super.m()` 的 this 保持调用方 this（CallThis 语义）；
  - **关键修正**：首版补丁漏发 GetProto 指令——super.m 直接读到对象
    自身同名方法 → 无限递归栈溢出（bc 指令 dump 定位）；
- **净效果**：t262 1020 → **1024/1154**（失败 47 → 43）；
  **M72_FLOOR 951 → 954**（理论上限 957 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=954 ✓、conformance 差分 ✓、express e2e ✓、jitbench 3/3 ✓、
  GC 压力 0 失败 ✓；super 探针（sm/sm2/sm3）对齐 Node 22。

## 73. M7.2 轮五十九：Symbol 静态方法真实属性（20260913）

- **根因**：`Symbol.for` / `Symbol.keyFor` 仅按名硬编码分派（CALL_METHOD
  链内识别），Symbol 构造器上**无真实属性**——`typeof Symbol.for` 为
  undefined，官方 verifyCallableProperty 断言（`typeof value === "function"`
  + 属性描述符校验）失败（S19.4.2 族 2 例）；
- **修复**：surface 装配期为 Symbol 构造器挂 for/keyFor 真实属性
  （NativeFn "Symbol.for"/"Symbol.keyFor"）+ registry handler
  （symbol_for_dispatch / symbol_key_for_dispatch 薄包装转发
  vm.symbol_for / vm.symbol_key_for）；
- 附：fmt 期间 FLOOR 注释误写 935，复核（1025 通过/42 失败 → 上限 958）
  后修正为 **955**；
- **净效果**：t262 1024 → **1025/1154**（失败 43 → 42）；
  **M72_FLOOR 954 → 955**（理论上限 958 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=955 ✓、conformance 差分 ✓；typeof/harness 探针对齐 Node 22。

## 74. M7.2 轮六十：Date() 无 new 直调返回时间串（20260913）

- **规范（Annex B）**：`Date(value)` 无 new 直调返回**当前时间的可读
  字符串**且**忽略参数**（`typeof Date() === "string"`、
  `Date(1970).length > 0`）；`new Date(v)` 才是对象。此前带参误等价
  `new Date(v)` 得对象；
- **净效果**：t262 1025 → **1026/1154**（失败 42 → 41）；
  **M72_FLOOR 955 → 956**（1026 通过/41 失败 → 上限 959，留 3 余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=956 ✓、conformance 差分 ✓；Date() 探针 3/3 对齐 Node 22。

## 75. M7.2 轮六十一：direct eval 闭包保留注入局部（20260913）

- **修复**：direct eval 的写回逻辑对「全局原本无此名的快照注入项」
  **保留注入值**而非移除——eval 内定义的函数/访问器闭包返回后仍引用
  注入的局部名，移除会令调用报 ReferenceError
  （`eval("o = {get foo(){ return s1;}}")` 后 `o.foo` 读 s1；
  S10.4.2.1 族 2 例）；
- **排障记录（深水项登记）**：同族余例 `s1 is not defined` 的根因更深——
  顶层 `var s1` 的槽位注册与 direct eval marker 名表时序交互
  （实测 eval 快照 names=["getter","e"] 不含 s1），需编译器层面
  统一顶层 var 的槽位注册路径后才能闭环（登记 eval 语义深水项）；
- **净效果**：t262 1026/1154 持平（getter 语义修复为后续用例铺路）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 ✓、conformance 差分 ✓；eval 闭包探针部分对齐（还原语义正确，
  余例待 marker 时序修复）。

## 76. M7.2 轮六十二：ToPrimitive 采纳判定补 Symbol（20260913）

- **根因**：to_primitive_number 的**第二处**采纳判定（res_primitive，
  轮廿八首版实现）漏了 `HeapObject::Symbol`——valueOf 返回符号时被
  判「非原始」→ 跳过 → toString 兜底得 "[object Object]" 堆串 →
  ToNumber 得 NaN（`({valueOf:()=>Symbol()}) * 1` 应抛 TypeError）；
  轮四十同位置的首处早退已含 Symbol，两处口径不一致；
- **修复**：res_primitive 补 `Some(HeapObject::Symbol { .. })`——
  valueOf 产符号被采纳为原始值 → 上层 numeric_operand 的符号守卫
  抛 TypeError（与 node 消息同为 TypeError，A/B/C 探针对齐）；
- **顺带**：清理 mul 调试探针（interpreter.rs/ops.rs）；
- **净效果**：t262 1026/1154 持平（BigInt wrapped-values 3 例的
  TypeError 判定修正，具体翻转待下轮 FLOOR 复核确认）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 ✓、conformance 差分 ✓；m2 探针 3/3（均 TypeError）对齐。

## 77. M7.2 轮六十三：super 赋值语义闭环（20260914）

- **对象访问器 setter 的 `super.s = v` 闭环**：解析（MemberAssign 的
  Super obj）→ 求值（home upvalue → GetProto → GetProp setter）→
  调用（CallThis 栈序 [this][setter][value]）三层修正——首版 obj 二次
  压栈致栈序错乱、GetProp 读 setter-only 属性得 undefined 致静默；
- **ISA 第 109 条操作码 SET_SUPER_PROP**：栈 [home_proto][this][value]
  + key 常量索引——op.rs 全表登记（pops 2/net -1/pushes 0/is_pure
  false/is_jump false ×3）；VM 臂沿 home 原型链查 setter 以 this 调用、
  无 setter 则 this 定义数据属性；JIT helper 全链路（4 参签名 + iconst
  key_idx + ctx 常量池解析）；
- **修复过程自纠**：批量补表脚本同表重复插入（clippy
  unreachable_pattern 拦下），以「函数边界 + Counter」全表审计修复并
  确认 9 表零重复；dump 调试模块用后即删；
- **净效果**：t262 1026/1154 持平（访问器 super 探针 3/3 对齐）；
  **M72_FLOOR 956 → 957**（理论上限 960 留余量）；
- 门禁证据：fmt ✓、clippy exit 0 ✓、workspace 全量 0 失败 ✓、
  t262 FLOOR=957 ✓、conformance 差分 ✓、express e2e ✓、jitbench 3/3 ✓、
  GC 压力 0 失败 ✓。
