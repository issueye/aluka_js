'use strict';
// 单元测试：业务规则（内存存储桩，无 IO）
const test = require('node:test');
const assert = require('node:assert/strict');

const { TaskService, fingerprint, STATUSES } = require('../src/service');
const { ValidationError, NotFoundError, ConflictError } = require('../src/errors');

/** 内存存储桩：实现 TaskStore 的同一接口面 */
class MemoryStore {
  constructor() {
    this.items = [];
  }
  all() {
    return this.items.map((item) => Object.assign({}, item));
  }
  get(id) {
    const found = this.items.find((item) => item.id === id);
    return found ? Object.assign({}, found) : null;
  }
  insert(task) {
    this.items.push(Object.assign({}, task));
    return task;
  }
  replace(id, task) {
    const index = this.items.findIndex((item) => item.id === id);
    if (index < 0) return null;
    this.items[index] = Object.assign({}, task);
    return task;
  }
  remove(id) {
    const index = this.items.findIndex((item) => item.id === id);
    if (index < 0) return null;
    return this.items.splice(index, 1)[0];
  }
  get size() {
    return this.items.length;
  }
}

function makeService() {
  const store = new MemoryStore();
  const service = new TaskService(store, { clock: () => Date.UTC(2026, 8, 15, 9, 0, 0) });
  return { store, service };
}

test('指纹稳定且区分大小写内容', () => {
  assert.equal(fingerprint('写周报'), fingerprint('写周报'));
  assert.notEqual(fingerprint('写周报'), fingerprint('写周报 '));
  assert.equal(fingerprint('abc').length, 10);
});

test('创建任务：默认值与派生字段', () => {
  const { service } = makeService();
  const task = service.create({ title: '  评审 PR  ' });
  assert.equal(task.id, 'T001');
  assert.equal(task.title, '评审 PR', '标题应 trim');
  assert.equal(task.status, 'todo');
  assert.equal(task.priority, 'normal');
  assert.deepEqual(task.tags, []);
  assert.equal(task.createdAt, '2026-09-15T09:00:00.000Z');
  assert.equal(task.fingerprint, fingerprint('评审 PR'));
});

test('创建任务：校验失败抛 ValidationError 并带字段明细', () => {
  const { service } = makeService();
  assert.throws(
    () => service.create({ title: '  ', priority: 'urgent' }),
    (err) => {
      assert.ok(err instanceof ValidationError);
      assert.equal(err.status, 400);
      assert.equal(err.details.length, 2);
      assert.deepEqual(
        err.details.map((d) => d.field).sort(),
        ['priority', 'title']
      );
      return true;
    }
  );
});

test('创建任务：标题超长被拒', () => {
  const { service } = makeService();
  assert.throws(() => service.create({ title: 'x'.repeat(81) }), ValidationError);
  assert.equal(service.create({ title: 'x'.repeat(80) }).title.length, 80);
});

test('重复未完成任务被拒（ConflictError）', () => {
  const { service } = makeService();
  service.create({ title: '写周报' });
  assert.throws(() => service.create({ title: '写周报' }), ConflictError);

  const done = service.update('T001', { status: 'done' });
  assert.equal(done.status, 'done');
  assert.equal(service.create({ title: '写周报' }).id, 'T002', '完成后可重建同名任务');
});

test('更新不存在任务抛 NotFoundError', () => {
  const { service } = makeService();
  assert.throws(() => service.update('T404', { status: 'doing' }), NotFoundError);
  assert.throws(() => service.get('T404'), NotFoundError);
  assert.throws(() => service.remove('T404'), NotFoundError);
});

test('更新状态与标题（含指纹重算）', () => {
  const { service } = makeService();
  service.create({ title: 'a' });
  const updated = service.update('T001', { status: 'doing', title: 'b' });
  assert.equal(updated.status, 'doing');
  assert.equal(updated.title, 'b');
  assert.equal(updated.fingerprint, fingerprint('b'));
  assert.equal(updated.createdAt, updated.updatedAt);
});

test('列表过滤与排序（status 升序 + id 升序）', () => {
  const { service } = makeService();
  service.create({ title: 'a', tags: ['x'] });
  service.create({ title: 'b', priority: 'high' });
  service.create({ title: 'c', tags: ['x', 'y'] });
  service.update('T002', { status: 'done' });

  assert.deepEqual(service.list({}).map((t) => t.id), ['T002', 'T001', 'T003']);
  assert.deepEqual(service.list({ status: 'todo' }).map((t) => t.id), ['T001', 'T003']);
  assert.deepEqual(service.list({ tag: 'x' }).map((t) => t.id), ['T001', 'T003']);
  assert.deepEqual(service.list({ priority: 'high' }).map((t) => t.id), ['T002']);
  assert.deepEqual(service.list({ limit: 2 }).map((t) => t.id), ['T002', 'T001']);
});

test('统计：计数 / 优先级 / 标签排序 / 完成率', () => {
  const { service } = makeService();
  service.create({ title: 'a', priority: 'high', tags: ['x', 'y'] });
  service.create({ title: 'b', tags: ['x'] });
  service.create({ title: 'c' });
  service.update('T001', { status: 'done' });

  const stats = service.stats();
  assert.deepEqual(stats.counts, { todo: 2, doing: 0, done: 1 });
  assert.deepEqual(stats.byPriority, { low: 0, normal: 2, high: 1 });
  assert.deepEqual(stats.topTags, [
    { tag: 'x', count: 2 },
    { tag: 'y', count: 1 },
  ]);
  assert.equal(stats.completion, 33);
  assert.equal(stats.total, 3);
});

test('空看板完成率为 0（不产生 NaN）', () => {
  const { service } = makeService();
  const stats = service.stats();
  assert.equal(stats.total, 0);
  assert.equal(stats.completion, 0);
  assert.deepEqual(stats.topTags, []);
});

test('状态枚举与非法状态拦截', () => {
  assert.deepEqual(STATUSES, ['todo', 'doing', 'done']);
  const { service } = makeService();
  service.create({ title: 'a' });
  assert.throws(() => service.update('T001', { status: 'archived' }), ValidationError);
  assert.throws(() => service.create({ title: 'b', status: 'archived' }), ValidationError);
});
