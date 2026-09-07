// Node.js 官方测试风格子集：Phase 3 P1 Node 模块。
const assert = require('node:assert');

// perf_hooks。
const { performance } = require('node:perf_hooks');
const t0 = performance.now();
assert.strictEqual(typeof performance.timeOrigin, 'number');
assert.ok(performance.now() >= t0);

// v8.serialize/deserialize。
const v8 = require('node:v8');
const buf = v8.serialize({ a: 1, b: 'x' });
const back = v8.deserialize(buf);
assert.strictEqual(back.a, 1);
assert.strictEqual(back.b, 'x');

// timers/promises.setTimeout。
const { setTimeout: ts } = require('node:timers/promises');
ts(5, 'resolved').then((v) => {
  assert.strictEqual(v, 'resolved');
  console.log('PASS p1');
});
