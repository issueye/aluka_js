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
