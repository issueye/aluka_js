# 2026-09-09 · 每日 TODO（M5 续轮 round7：M5.1 结构化克隆与 worker 传值语义）

> 总 TODO 见 [../README.md](../README.md)；上一轮（M5.2 P0 关闭）见 [./README-round6.md](./README-round6.md)。

**当前里程碑**：M5 多线程并发与系统级扩展（遗留清理）　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

## 1. 本轮目标（可判定完成态）

评审建议顺序第三项——**M5.1 结构化克隆**（现传值为 JSON 往返：对象键排序、
undefined→null、ArrayBuffer/TypedArray/函数→null、无 transfer、循环引用
崩）：

1. 实现规范级**结构化克隆算法**（HTML StructuredSerialize：基本类型/Date/
   RegExp/Map/Set/ArrayBuffer/TypedArray/DataView/普通对象与数组、**循环
   引用**、原型简化为普通对象）；
2. MessagePort/MessageChannel/BroadcastChannel 传值通路从 JSON 切换为
   结构化克隆；
3. **transfer list**：ArrayBuffer transfer（原缓冲 detach、零拷贝移交）与
   `markAsUntransferable`（登记名单内 ArrayBuffer 报 DataCloneError）；
4. 小项顺手：`new MessagePort()` 抛 ERR_ILLEGAL_CONSTRUCTOR、模块头注释
   修订（threadId/克隆口径）；
5. 差分用例（20-m5 扩展：循环引用/Map/Set/Date/RegExp/typed array/
   transfer/DataCloneError）Node 22.23.1 逐字对拍 + 门禁全绿 + 登记
   （M5.1 完成度复核：eval worker/ref-unref 等仍登记）。

## 2. 待办清单

| # | 任务项 | 状态 | 证据 |
|---|---|:---:|---|
| 1 | 勘察现传值通路与数据结构 | `[x]` | worker.rs 契约 + 收发点全景 |
| 2 | 结构化克隆算法（克隆表 + 循环引用 + 类型面） | `[x]` | worker_clone.rs(新,~700 行) |
| 3 | transfer list + markAsUntransferable | `[x]` | st-probe 对拍 + 单测 |
| 4 | 端口/注释小项 + 差分用例 + 门禁 | `[x]` | 见下 |

## 2b. 交付摘要（M5.1 结构化克隆闭环）

- **新模块 `worker_clone.rs`**:自描述字节格式(ALSC1 magic + tag 流),
  支持 undefined/null/布尔/数字/字符串/bigint/Date/RegExp/数组/普通对象/
  Map/Set/ArrayBuffer/TypedArray/DataView;对象图引用表(循环/共享还原);
  函数/Symbol/Promise/Proxy → DataCloneError;transfer list(ArrayBuffer 与
  视图整缓冲移交 + 源 detach:data 清空、视图 length 归零、DataView 属性读
  抛 TypeError——property.rs 合成面适配);markAsUntransferable 登记表;
- **通路替换**(worker_threads.rs):parentPort/Worker.postMessage 真实线程
  通道与 workerData 从 JSON 往返切换为结构化克隆字节(base64 承载,通道
  String 契约与 runtime 层零改动);同进程端口(MessageChannel/BC/proc
  回退)同线程克隆=serialize+deserialize(对齐 Node 克隆语义);
- **顺带修复**:worker 事件循环仅按宏任务/事件源保活——**纯消息应答型
  worker(无定时器)误退出**,补 parentPort 'message' 监听器保活(Node 语义);
  `new MessagePort()` 抛 ERR_ILLEGAL_CONSTRUCTOR;
- **差分用例** `25-m5-structured-clone.cjs`(conformance,自动入列)——
  22 行输出与 Node 22.23.1 **逐字一致**(类型面/循环与共享引用/transfer/
  detach 后 ab 0/ta 0/dv 抛 TypeError/错误 name/unsupported 文本×2);
- **回归**:20-m5/21-m5 全绿;conformance 25/25(+2 invalid);全量门禁
  561 passed / 0 failed;fmt/clippy 零告警。

### 简化口径与引擎缺口登记（下轮）

- 多视图共享同一 ArrayBuffer 的对象图克隆时各视图独立复制(共享关系不
  保留——V8 保留,登记);MessagePort 进 transfer list 未实现(抛
  unsupported,Node 支持端口转移);workerData 复杂对象复验(通路与消息
  同款,风险同已验,补测列下轮);
- 引擎既有面缺口(与克隆无关,本次对拍绕行):Date 实例方法(getTime/
  toISOString)分派缺失、原型方法属性读面(join/constructor/instanceof)、
  WeakMap 无实现、Set 键字符串化(3 与 '3' 同键)、Array.from 缺失。

## 3. 门禁结果（全绿）

```bash
cargo fmt --all --check                        # FMT-OK
cargo clippy --workspace --all-targets --all-features -- -D warnings  # 零警告
cargo test --workspace --all-features          # 561 passed / 0 failed
```

## 4. 提交

```bash
git commit -m "feat(m5.1): worker 结构化克隆——类型面/循环引用/transfer/detach + 纯消息 worker 保活"
```

## 5. 遗留/下轮

- M5.4 Timer Mock + CLI 运行器;M5.1 余项:MessagePort transfer、视图共享
  克隆、workerData 复验、eval worker/ref-unref/start;M5.2 IPC 面;
- 引擎面:Date 实例方法/原型属性面/WeakMap/Set 键语义。
