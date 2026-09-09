// M5.2 差分用例：cluster 多进程 + HTTP 端口共享。
// 主进程 fork 2 个 worker，各自 http server 监听同一端口（SO_REUSEADDR/
// REUSEPORT 共享），主进程 fetch 探活后 disconnect 收尾。
const cluster = require('node:cluster');
const PORT = 3457;

if (cluster.isPrimary) {
  for (let i = 0; i < 2; i++) {
    cluster.fork();
  }
  const tryFetch = (n) => {
    fetch(`http://127.0.0.1:${PORT}/`)
      .then(async (r) => {
        const body = await r.text();
        console.log('probe:', r.status, body);
        cluster.disconnect();
      })
      .catch(() => {
        if (n > 0) {
          setTimeout(() => tryFetch(n - 1), 100);
        } else {
          console.log('probe: failed');
          cluster.disconnect();
        }
      });
  };
  setTimeout(() => tryFetch(30), 200);
} else {
  const http = require('node:http');
  http
    .createServer((req, res) => {
      res.end('worker-ok');
    })
    .listen(PORT);
}
