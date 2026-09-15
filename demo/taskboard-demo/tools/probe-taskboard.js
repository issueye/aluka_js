'use strict';
// 用项目真实类隔离复现（错误属性 + TaskServer.listen）
const path = require('node:path');
const fs = require('node:fs');
const { TaskStore } = require('../src/store');
const { TaskService } = require('../src/service');
const { TaskServer } = require('../src/http-server');

const DATA = path.join('.probe-data', 'tasks.json');
fs.rmSync('.probe-data', { recursive: true, force: true });

const store = new TaskStore(DATA).load();
const service = new TaskService(store, { clock: () => Date.UTC(2026, 8, 15, 9, 0, 0) });
service.create({ title: '写周报' });

// ① 业务错误属性
try {
  service.create({ title: '写周报' });
  console.log('conflict: NO_ERROR');
} catch (err) {
  console.log(
    'conflict: typeof=' + typeof err,
    'name=' + JSON.stringify(err && err.name),
    'code=' + JSON.stringify(err && err.code),
    'status=' + JSON.stringify(err && err.status),
    'message=' + JSON.stringify(err && err.message)
  );
}

// ② TaskServer.listen 是否 resolve
console.log('step: constructing TaskServer');
const server = new TaskServer(service, { logger: null, maxBodyBytes: 2048 });
console.log('step: constructed, server.server=', typeof server.server);
console.log('step: calling listen');
server
  .listen(0, '127.0.0.1')
  .then((address) => {
    console.log('listen resolved:', address.port > 0);
    return server.close();
  })
  .then(() => {
    console.log('PROBE7_DONE');
  })
  .catch((err) => {
    console.log('PROBE7_FAILED:', err && err.message);
    console.log('PROBE7_DONE');
  });
