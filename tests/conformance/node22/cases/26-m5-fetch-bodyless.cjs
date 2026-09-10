// M5 回归用例：fetch 响应定界——bodyless 状态（204）与 Content-Length 并发多连接。
//
// 锁定两个已实测缺陷（见 .work/TODO/20260910/README-m5-review.md §3.2）：
//   1. 204/304 这类无 body 响应曾落回读超时兜底 → 每请求空等 10s（实测 10013ms）；
//   2. 并发多连接曾各自叠加 10s（实测双 fetch 20s）。
// 服务端放在 fork 出的子进程（跨进程），因为 aluka 的 fetch 是同步阻塞实现，
// 同进程自请求会互锁——这与 21-m5 的进程模型一致。
//
// 输出确定性纪律：只打印状态码/长度/粗粒度二值化计时（阈值 5s，正常 ~5ms、
// 缺陷态 10013ms，量级差 3 个数量级），不做精确计时或顺序依赖断言。
const cluster = require('node:cluster');
const PORT = 3471;

if (cluster.isPrimary) {
  cluster.fork();
  const run = async () => {
    const one = async (path) => {
      const t0 = Date.now();
      const r = await fetch(`http://127.0.0.1:${PORT}${path}`);
      const body = await r.text();
      const fast = Date.now() - t0 < 5000;
      return `${path} status=${r.status} len=${body.length} fast=${fast}`;
    };
    // 并发发起两条连接（Promise.all 的结果顺序按入参数组，与完成顺序无关）
    const results = await Promise.all([one('/text'), one('/no-content')]);
    console.log(results[0]);
    console.log(results[1]);
    cluster.disconnect();
  };
  // 就绪探测（服务端跨进程启动有时序，重试由 21-m5 口径沿用）
  const ready = (n) => {
    fetch(`http://127.0.0.1:${PORT}/text`)
      .then(async (r) => {
        await r.text();
        run();
      })
      .catch(() => {
        if (n > 0) {
          setTimeout(() => ready(n - 1), 100);
        } else {
          console.log('probe: failed');
          cluster.disconnect();
        }
      });
  };
  setTimeout(() => ready(30), 200);
} else {
  const http = require('node:http');
  http
    .createServer((req, res) => {
      if (req.url === '/no-content') {
        // 无 body：不得有 Content-Length，也不得 chunked
        res.statusCode = 204;
        res.end();
        return;
      }
      res.setHeader('Content-Type', 'text/plain');
      res.end('ok');
    })
    .listen(PORT);
}
