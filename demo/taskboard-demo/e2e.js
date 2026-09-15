#!/usr/bin/env node
'use strict';
// 端到端：真实文件存储 + 真实 HTTP 往返 + 真实 npm 依赖 + 错误路径 + 并发
// 输出为确定性文本（无时间戳/绝对路径/随机值），供 aluka 与 node 逐行对拍。
// 入口位于项目根（运行期镜像以入口目录为 root；入口若在 scripts/ 子目录，
// 则 ../src/* 会被编译到 _ext/ 且无法解析——见 .work/TODO/20260915 §遗留）。
const fs = require('node:fs');
const path = require('node:path');

const { loadConfig } = require('./src/config');
const { createLogger } = require('./src/logger');
const { TaskStore } = require('./src/store');
const { TaskService, fingerprint } = require('./src/service');
const { TaskServer } = require('./src/http-server');
const { request, section, line, fixedClock, ROOT, DATA_DIR, DATA_FILE } = require('./test/helpers/e2e-lib');

async function main() {
  // 看门狗：任何环节挂起都要以确定的方式失败退出（不能吊住事件循环）
  const watchdog = setTimeout(() => {
    process.stderr.write('E2E_TIMEOUT\n');
    process.exit(2);
  }, 15000);

  // 0. 清理历史数据（真实文件 IO）
  fs.rmSync(DATA_DIR, { recursive: true, force: true });
  line('cleanup:', fs.existsSync(DATA_DIR) ? 'FAILED' : 'ok');

  // 1. 配置与日志
  section('config');
  const config = loadConfig(ROOT);
  line('host:', config.host, 'port:', config.port, 'logLevel:', config.logLevel);
  line('dataFileBasename:', path.basename(config.dataFile));
  line('configKeys:', Object.keys(config).sort().join(','));

  const logger = createLogger({ level: 'info', sink: () => {}, sinkError: () => {} });
  line('logger.info:', logger.info('boot', { pid: 'x', module: 'e2e' }));
  line('logger.debug(filtered):', String(logger.debug('hidden')));
  line('logger.with:', logger.with({ module: 'http' }).info('req', { status: 200 }));

  // 2. 真实 npm 依赖（node_modules/ms，由 npm / aluka npm 安装）
  section('dependency');
  try {
    const ms = require('ms');
    line('ms(60000):', ms(60000));
    line('ms("2d"):', ms('2d'));
    line('ms(1500,{long:true}):', ms(1500, { long: true }));
  } catch (err) {
    line('ms: REQUIRE_FAILED', (err && err.code) || (err && err.name) || String(err));
  }

  // 3. 存储 + 业务（真实落盘）
  section('store');
  const store = new TaskStore(DATA_FILE).load();
  const service = new TaskService(store, { logger, clock: fixedClock });

  const created = [];
  created.push(service.create({ title: '写周报', priority: 'high', tags: ['doc', 'weekly'] }));
  created.push(service.create({ title: '评审 PR', tags: ['review'] }));
  created.push(service.create({ title: '修复登录缺陷', priority: 'low' }));
  const bytes = store.save();
  line('created:', created.map((t) => `${t.id}:${t.status}`).join(' '));
  line('fingerprint(写周报):', fingerprint('写周报'));
  line('fileBytes>0:', bytes > 0, 'fileExists:', fs.existsSync(DATA_FILE));
  line('reloadSize:', new TaskStore(DATA_FILE).load().size);

  try {
    service.create({ title: '写周报' });
    line('duplicate: NO_ERROR');
  } catch (err) {
    line('duplicate:', err.name, err.code, err.status);
  }

  try {
    service.create({ title: '   ', priority: 'urgent' });
    line('validation: NO_ERROR');
  } catch (err) {
    line('validation:', err.name, err.status, JSON.stringify(err.details));
  }

  // 4. 状态流转与统计
  section('service');
  service.update(created[1].id, { status: 'doing' });
  service.update(created[0].id, { status: 'done' });
  store.save();
  const stats = service.stats();
  line('counts:', JSON.stringify(stats.counts), 'completion:', stats.completion);
  line('topTags:', JSON.stringify(stats.topTags));
  line('list(done):', service.list({ status: 'done' }).map((t) => t.id).join(','));
  line('list(tag=review):', service.list({ tag: 'review' }).map((t) => t.id).join(','));
  line('list(limit=2):', service.list({ limit: 2 }).map((t) => t.id).join(','));
  try {
    service.get('T999');
    line('get(missing): NO_ERROR');
  } catch (err) {
    line('get(missing):', err.name, err.code, err.status);
  }

  await runHttp(service, store);
  line('\nE2E_DONE');
  clearTimeout(watchdog);
}

async function runHttp(service, store) {
  section('http');
  const server = new TaskServer(service, { logger: null, maxBodyBytes: 2048 });
  const address = await server.listen(0, '127.0.0.1');
  const port = address.port;
  line('listening:', typeof port === 'number' && port > 0);

  const health = await request(port, 'GET', '/health');
  line('GET /health:', health.status, JSON.stringify(health.body));

  const list = await request(port, 'GET', '/tasks?status=todo');
  line('GET /tasks?status=todo:', list.status, list.body.count);

  const post = await request(port, 'POST', '/tasks', { title: '部署 v1', tags: ['ops'] });
  line('POST /tasks:', post.status, post.body.id, post.body.status);

  const one = await request(port, 'GET', `/tasks/${post.body.id}`);
  line('GET /tasks/:id:', one.status, one.body.title);

  const patch = await request(port, 'PATCH', `/tasks/${post.body.id}`, { status: 'doing' });
  line('PATCH /tasks/:id:', patch.status, patch.body.status);

  const bad = await request(port, 'POST', '/tasks', { title: '' });
  line('POST /tasks (invalid):', bad.status, bad.body.error, bad.body.details.length);

  const missing = await request(port, 'GET', '/tasks/T999');
  line('GET /tasks/T999:', missing.status, missing.body.error);

  const tooLarge = await request(port, 'POST', '/tasks', { title: 'x'.repeat(4096) });
  line('POST /tasks (too large):', tooLarge.status, tooLarge.body.error);

  const notAllowed = await request(port, 'PUT', '/tasks');
  line('PUT /tasks:', notAllowed.status, notAllowed.body.error);

  section('concurrency');
  const batch = await Promise.all(
    Array.from({ length: 10 }, (_, i) => request(port, 'GET', `/tasks/T00${(i % 3) + 1}`))
  );
  line('concurrent statuses:', batch.map((r) => r.status).join(','));
  line('concurrent ids:', batch.map((r) => r.body.id).join(','));

  section('teardown');
  await server.close();
  line('serverClosed: ok');
  const persisted = JSON.parse(fs.readFileSync(DATA_FILE, 'utf8'));
  line('persistedCount:', persisted.length);
  line('persistedIds:', persisted.map((t) => t.id).join(','));
  line('createdAtStable:', persisted.every((t) => t.createdAt === '2026-09-15T09:00:00.000Z'));
}

main().catch((err) => {
  process.stderr.write(`E2E_FAILED: ${err && err.stack ? err.stack : err}\n`);
  process.exit(1);
});