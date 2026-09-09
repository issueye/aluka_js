# Aluka 工程日志 — 2026-09-08

## 完成工作

### M5.1: worker_threads 真实跨物理线程 ✅
- **全仓静态表线程局部化**：75 处 Mutex 静态变量 → thread_local
- **新模块 `aluka-vm::worker`**：WorkerBridge/WorkerEvent/WorkerThreadIo
- **`new Worker` 双路径**：装配钩子存在 → spawn 物理线程；否则回落伪 worker
- **worker 线程主体**（aluka-runtime）：字节码直载/源码编译 → 独立 Vm → 事件循环
- **terminate 协作式**：旗标 + 即时派发 exit(1)；Exit(0) 滞留丢弃
- **差分用例 20-m5-worker-threads.cjs** 通过，两模式与 Node 22 一致

### M5.2: cluster 进程池 ✅
- **socket2 共享绑定**：SO_REUSEADDR/REUSEPORT 支持多进程同端口
- **`worker.destroy` 真实 kill**：委托 child 对象 kill 方法，退出事件清理 CHILDREN 表
- **fork 脚本路径修复**：优先 `vm.entry_file`（字节码模式 argv[1] 含子命令 "run"）
- **fetch 报文 CRLF 修复**：HTTP/1.1 行尾 LF → CRLF；响应 CRLF 分帧
- **fetch 规范语义**：网络错误转 rejected promise 非同步抛出
- **差分用例 21-m5-cluster-http.cjs** 通过，两模式与 Node 22 一致

### M5.3: node:sqlite 生产级支持 ✅
- **DatabaseSync.isTransaction** 数据属性实时同步
- **BLOB 读出纯 Uint8Array**（非 Buffer），对齐 Node 22
- Node 22 差分验证：命名参数、手动事务 COMMIT/ROLLBACK、bigint、列序、NULL

### M5.4: node:test concurrency 选项 ✅
- **TestOpts/TestNode/SuiteNode 增加 concurrent 字段**
- **run_suite 批量分组**：连续并发标记用例 → `run_concurrent_batch`
- **并发批调度**：start 阶段全部启动（beforeEach + invoke），settle 阶段微任务/宏任务交错排空
- **e2e 断言**：concurrent=true 时 b-start 出现在 a-end 之前（批启动语义）

## 门禁
- cargo fmt —all —check: OK
- cargo clippy — -D warnings: 0 errors
- cargo test —workspace —all-features: 544 passed, 0 failed
