'use strict';
// 集成测试：HTTP API（真实监听 127.0.0.1:0 + 真实往返）
const test = require('node:test');
const assert = require('node:assert/strict');

const { TaskStore } = require('../src/store');
const { TaskService } = require('../src/service');
const { TaskServer } = require('../src/http-server');
const fs = require('node:fs');
const path = require('node:path');

// 每个测试文件用**独立**数据目录：node:test 以并行子进程跑各文件，
// 共用目录会互相删除/写入（ENOTEMPTY / EPERM 竞态）
const TMP_FILE = path.join(__dirname, '..', '.data-test-api', 'api.json');

function fixedClock() {
  return Date.UTC(2026, 8, 15, 9, 0, 0);
}

function request(port, method, urlPath, body) {
  const http = require('node:http');
  return new Promise((resolve, reject) => {
    // 字符串按原样发送（用于「非法 JSON」用例），其余对象走 JSON 序列化
    const payload =
      body === undefined ? null : typeof body === 'string' ? body : JSON.stringify(body);
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: urlPath,
        method,
        headers: payload
          ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) }
          : {},
      },
      (res) => {
        const chunks = [];
        res.on('data', (chunk) => chunks.push(chunk));
        res.on('end', () => {
          const text = Buffer.concat(chunks).toString('utf8');
          resolve({
            status: res.statusCode,
            type: res.headers['content-type'],
            location: res.headers.location,
            body: text === '' ? null : JSON.parse(text),
          });
        });
      }
    );
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

/** 启动一个隔离的服务实例，返回 { port, close } */
async function startServer() {
  fs.rmSync(path.dirname(TMP_FILE), { recursive: true, force: true });
  const store = new TaskStore(TMP_FILE).load();
  const service = new TaskService(store, { clock: fixedClock });
  const server = new TaskServer(service, { maxBodyBytes: 1024 });
  const address = await server.listen(0, '127.0.0.1');
  return {
    port: address.port,
    close: () => server.close(),
  };
}

test('健康检查与 404 路由', async () => {
  const srv = await startServer();
  try {
    const health = await request(srv.port, 'GET', '/health');
    assert.equal(health.status, 200);
    assert.deepEqual(health.body, { ok: true, uptimeClass: 'static' });
    assert.equal(health.type, 'application/json');

    const missing = await request(srv.port, 'GET', '/nope');
    assert.equal(missing.status, 404);
    assert.equal(missing.body.error, 'NOT_FOUND');
    assert.equal(missing.body.path, '/nope');
  } finally {
    await srv.close();
  }
});

test('任务 CRUD 全链路（含 201 + Location 头）', async () => {
  const srv = await startServer();
  try {
    const created = await request(srv.port, 'POST', '/tasks', {
      title: '部署 v1',
      priority: 'high',
      tags: ['ops'],
    });
    assert.equal(created.status, 201);
    assert.equal(created.body.id, 'T001');
    assert.equal(created.body.status, 'todo');
    assert.equal(created.location, '/tasks/T001');

    const fetched = await request(srv.port, 'GET', '/tasks/T001');
    assert.equal(fetched.status, 200);
    assert.equal(fetched.body.title, '部署 v1');

    const patched = await request(srv.port, 'PATCH', '/tasks/T001', { status: 'doing' });
    assert.equal(patched.status, 200);
    assert.equal(patched.body.status, 'doing');

    const listed = await request(srv.port, 'GET', '/tasks?status=doing');
    assert.equal(listed.status, 200);
    assert.equal(listed.body.count, 1);

    const removed = await request(srv.port, 'DELETE', '/tasks/T001');
    assert.equal(removed.status, 200);
    assert.equal(removed.body.removed, 'T001');

    const gone = await request(srv.port, 'GET', '/tasks/T001');
    assert.equal(gone.status, 404);
  } finally {
    await srv.close();
  }
});

test('请求体错误：非法 JSON / 超限 / 非 JSON 体', async () => {
  const srv = await startServer();
  try {
    const notJson = await request(srv.port, 'POST', '/tasks', 'raw-text');
    assert.equal(notJson.status, 400);
    assert.equal(notJson.body.error, 'VALIDATION_ERROR');
    assert.ok(Array.isArray(notJson.body.details));

    const tooLarge = await request(srv.port, 'POST', '/tasks', { title: 'x'.repeat(2000) });
    assert.equal(tooLarge.status, 413);
    assert.equal(tooLarge.body.error, 'PAYLOAD_TOO_LARGE');
  } finally {
    await srv.close();
  }
});

test('方法不允许（405）与统计端点', async () => {
  const srv = await startServer();
  try {
    const put = await request(srv.port, 'PUT', '/tasks');
    assert.equal(put.status, 405);
    assert.equal(put.body.error, 'METHOD_NOT_ALLOWED');

    await request(srv.port, 'POST', '/tasks', { title: 'a', tags: ['x'] });
    const stats = await request(srv.port, 'GET', '/stats');
    assert.equal(stats.status, 200);
    assert.equal(stats.body.total, 1);
    assert.deepEqual(stats.body.counts, { todo: 1, doing: 0, done: 0 });
  } finally {
    await srv.close();
  }
});

test('并发读请求（10 路并行）', async () => {
  const srv = await startServer();
  try {
    await request(srv.port, 'POST', '/tasks', { title: 'a' });
    await request(srv.port, 'POST', '/tasks', { title: 'b' });
    const results = await Promise.all(
      Array.from({ length: 10 }, (_, i) =>
        request(srv.port, 'GET', `/tasks/T00${(i % 2) + 1}`)
      )
    );
    assert.ok(results.every((r) => r.status === 200));
    assert.deepEqual(
      results.map((r) => r.body.id),
      ['T001', 'T002', 'T001', 'T002', 'T001', 'T002', 'T001', 'T002', 'T001', 'T002']
    );
  } finally {
    await srv.close();
  }
});
