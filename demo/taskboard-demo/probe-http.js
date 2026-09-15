'use strict';
// 根目录探针：项目真实 TaskServer 的 listen + 单次请求往返（逐步打点）
const path = require('node:path');
const http = require('node:http');
const fs = require('node:fs');
const { TaskStore } = require('./src/store');
const { TaskService } = require('./src/service');
const { TaskServer } = require('./src/http-server');

const DATA = path.join('.probe-http-data', 'tasks.json');
fs.rmSync('.probe-http-data', { recursive: true, force: true });

const watchdog = setTimeout(() => {
  console.log('WATCHDOG_TIMEOUT');
  process.exit(3);
}, 8000);

console.log('step1: build service');
const store = new TaskStore(DATA).load();
const service = new TaskService(store, { clock: () => Date.UTC(2026, 8, 15, 9, 0, 0) });
service.create({ title: 'probe' });

console.log('step2: build server');
const server = new TaskServer(service, { logger: null, maxBodyBytes: 2048 });
console.log('diag: typeof server =', typeof server);
console.log('diag: server instanceof TaskServer =', server instanceof TaskServer);
console.log('diag: typeof server.listen =', typeof server.listen);
console.log('diag: typeof server.close =', typeof server.close);
console.log('diag: typeof server.server =', typeof server.server);
console.log(
  'diag: typeof server.server.listen =',
  server.server ? typeof server.server.listen : 'n/a'
);
console.log('diag: ctor.name =', server.constructor && server.constructor.name);
console.log('diag: proto.ctor.name =', Object.getPrototypeOf(server.constructor).name);
console.log('step3: call listen');
server
  .listen(0, '127.0.0.1')
  .then((address) => {
    console.log('step4: listening, port>0 =', address.port > 0);
    return new Promise((resolve, reject) => {
      const req = http.request(
        { host: '127.0.0.1', port: address.port, path: '/health', method: 'GET' },
        (res) => {
          const chunks = [];
          res.on('data', (c) => chunks.push(c));
          res.on('end', () => resolve(`${res.statusCode} ${Buffer.concat(chunks).toString('utf8')}`));
        }
      );
      req.on('error', reject);
      req.end();
    });
  })
  .then((text) => {
    console.log('step5: response =', text);
    return server.close();
  })
  .then(() => {
    console.log('step6: closed');
    clearTimeout(watchdog);
    console.log('PROBE_HTTP_DONE');
  })
  .catch((err) => {
    console.log('PROBE_HTTP_FAILED:', err && err.name, '|', err && err.message);
    clearTimeout(watchdog);
    console.log('PROBE_HTTP_DONE');
    process.exit(1);
  });
