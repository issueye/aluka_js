'use strict';
// 单元测试：存储层（真实文件 IO）
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const { TaskStore } = require('../src/store');

const TMP = path.join(__dirname, '..', '.data-test-store', 'store.json');

function freshStore() {
  fs.rmSync(path.dirname(TMP), { recursive: true, force: true });
  return new TaskStore(TMP);
}

test('空存储的初始状态', () => {
  const store = freshStore();
  assert.equal(store.size, 0);
  assert.deepEqual(store.all(), []);
  assert.equal(store.load().size, 0);
});

test('保存后可从磁盘重新装载（原子落盘）', () => {
  const store = freshStore();
  store.insert({ id: 'T001', title: 'a', status: 'todo', tags: [] });
  store.insert({ id: 'T002', title: 'b', status: 'doing', tags: ['x'] });
  const bytes = store.save();

  assert.ok(bytes > 0);
  assert.ok(fs.existsSync(TMP));
  assert.equal(fs.existsSync(`${TMP}.tmp`), false, '临时文件应已改名');

  const reloaded = new TaskStore(TMP).load();
  assert.equal(reloaded.size, 2);
  assert.equal(reloaded.get('T002').title, 'b');
  assert.deepEqual(reloaded.get('T002').tags, ['x']);
});

test('get 返回副本（外部修改不影响内部状态）', () => {
  const store = freshStore();
  store.insert({ id: 'T001', title: 'a', tags: ['t'] });
  const copy = store.get('T001');
  copy.title = 'changed';
  copy.tags.push('mutated');
  assert.equal(store.get('T001').title, 'a');
  assert.deepEqual(store.get('T001').tags, ['t']);
});

test('replace / remove 的命中与未命中', () => {
  const store = freshStore();
  store.insert({ id: 'T001', title: 'a' });
  assert.equal(store.replace('T404', { id: 'T404' }), null);
  assert.equal(store.remove('T404'), null);

  const replaced = store.replace('T001', { id: 'T001', title: 'b' });
  assert.equal(replaced.title, 'b');
  assert.equal(store.get('T001').title, 'b');

  const removed = store.remove('T001');
  assert.equal(removed.id, 'T001');
  assert.equal(store.size, 0);
});

test('数据文件损坏时抛 TypeError', () => {
  fs.mkdirSync(path.dirname(TMP), { recursive: true });
  fs.writeFileSync(TMP, '{"not":"an array"}', 'utf8');
  assert.throws(() => new TaskStore(TMP).load(), TypeError);
});

test('空文件视为空存储', () => {
  fs.mkdirSync(path.dirname(TMP), { recursive: true });
  fs.writeFileSync(TMP, '   \n', 'utf8');
  assert.equal(new TaskStore(TMP).load().size, 0);
});
