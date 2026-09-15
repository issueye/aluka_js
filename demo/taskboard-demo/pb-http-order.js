'use strict';
// 验证 http 挂起是否与 §4.1 同源（顺序敏感）：http-server 先行 require
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');
require('./src/http-server'); // 先加载 http-server（对照 pb-http.js 的顺序）
const { TaskStore } = require('./src/store');
const { TaskService } = require('./src/service');
const { TaskServer } = require('./src/http-server');

const DATA = path.join('.probe-http-order-data', 'tasks.json');
fs.rmSync('.probe-http-order-data', { recursive: true, force: true });
const watchdog = setTimeout(() => {
  console.log('WATCHDOG_TIMEOUT');
  process.exit(3);
}, 8000);

const store = new TaskStore(DATA).load();
const service = new TaskService(store, { clock: () => Date.UTC(2026, 8, 15, 9, 0, 0) });
const server = new TaskServer(service, { logger: null, maxBodyBytes: 2048 });
console.log('listening...');
server
  .listen(0, '127.0.0.1')
  .then((address) => {
    console.log('LISTEN_OK port>0 =', address.port > 0);
    return server.close();
  })
  .then(() => {
    clearTimeout(watchdog);
    console.log('ORDER_PROBE_DONE');
  })
  .catch((err) => {
    console.log('ORDER_PROBE_FAILED:', err && err.message);
    clearTimeout(watchdog);
    process.exit(1);
  });
