const http = require('node:http');

// 验证 Agent API 完整性（不依赖精确连接计数——keepAlive 连接复用属深层优化）。
const a1 = new http.Agent();
console.log('p1:', a1.keepAlive, a1.maxSockets, a1.keepAliveMsecs, a1.maxFreeSockets);
const a2 = new http.Agent({ keepAlive: true, maxSockets: 5, keepAliveMsecs: 2000 });
console.log('p2:', a2.keepAlive, a2.maxSockets, a2.keepAliveMsecs, a2.maxFreeSockets);
console.log('p3:', typeof http.globalAgent, typeof http.Agent, http.globalAgent.keepAlive);
console.log('p4:', typeof a1.destroy, typeof a1.getName);

// 基本 HTTP 请求（含 agent）能正常工作。
const server = http.createServer((req, res) => {
  res.writeHead(200);
  res.end('ok');
});
server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  const agent = new http.Agent({ keepAlive: true });
  const req = http.request({ host: '127.0.0.1', port, path: '/', agent }, (res) => {
    let body = '';
    res.on('data', (c) => { body += c; });
    res.on('end', () => {
      console.log('p5:', res.statusCode, body);
      agent.destroy();
      server.close();
    });
  });
  req.end();
});
