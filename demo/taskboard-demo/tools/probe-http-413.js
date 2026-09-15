'use strict';
const http = require('node:http');
const { TaskStore } = require('./src/store');
const { TaskService } = require('./src/service');
const { TaskServer } = require('./src/http-server');
const svc = new TaskService(new TaskStore('.probe-413/tasks.json').load(), {});
const server = new TaskServer(svc, { maxBodyBytes: 512 });
const wd = setTimeout(function () { console.log('WATCHDOG'); process.exit(3); }, 3000);
server.listen(0, '127.0.0.1').then(function (a) {
  const payload = JSON.stringify({ title: 'x'.repeat(900) });
  const req = http.request({ host: '127.0.0.1', port: a.port, path: '/tasks', method: 'POST', headers: { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) } }, function (res) {
    const c = [];
    res.on('data', function (x) { c.push(x); });
    res.on('end', function () { console.log('413 RESPONSE:', res.statusCode, Buffer.concat(c).toString()); clearTimeout(wd); process.exit(0); });
  });
  req.on('error', function (e) { console.log('REQ ERROR:', e && e.message); process.exit(1); });
  req.write(payload);
  req.end();
}).catch(function (e) { console.log('LISTEN FAILED:', e && e.message); process.exit(1); });
