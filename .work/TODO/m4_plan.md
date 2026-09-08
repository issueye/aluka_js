# M4 · Web API 标准对齐（实现计划）

> 上级：[../README.md](../README.md) M4 节

## M4.1 规范级 Fetch API
- `fetch(url[, options]) -> Promise<Response>`：HTTP(S) GET/POST/PUT/DELETE
- `Response`：`.ok` `.status` `.statusText` `.headers` `.text()` `.json()` `.arrayBuffer()`
- `Headers`：append/delete/get/has/set/forEach/entries/keys/values
- `Request`：method/url/headers/body
- 实现层：复用 net.rs TcpStream 基建 + http wire.rs 解析；sync 请求→Promise resolve

## M4.2 AbortController 全系统级联动
- `AbortController()` → `{ signal: AbortSignal, abort(reason?) }`
- `AbortSignal`：`.aborted` `.reason` `.addEventListener('abort', cb)` `.throwIfAborted()`
- fetch 集成：`signal.aborted` 检查 + abort → reject(AbortError)

## M4.3 Web Streams 与 Node Streams 互通
- `ReadableStream` / `WritableStream` / `TransformStream` Web 标准 API
- `Readable.fromWeb(webReadable)` / `Readable.toWeb(nodeReadable)` 互通
- stream_web.rs 已有基础（417 行）

## 验收
- fetch GET/POST 集成测试与 Node.js 22 对拍
- AbortController 中断 fetch 请求测试
- Web Streams ↔ Node Streams pipe 互通测试

## 工作量估算
- M4.1 Fetch API：中等（复用 http TCP 基建 + Promise 包装）
- M4.2 AbortController：小（纯状态管理 + 事件分发）
- M4.3 Web Streams 互通：中等（stream_web.rs 已有基础）
