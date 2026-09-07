// Node.js 官方测试风格子集：assert 模块。
const assert = require('node:assert');

assert.ok(true);
assert.strictEqual(1 + 1, 2);
assert.notStrictEqual(1, '1');
assert.deepStrictEqual({ a: 1, b: [2, 3] }, { a: 1, b: [2, 3] });
assert.equal('5', 5); // 宽松相等

// throws 捕获同步异常。
assert.throws(() => { throw new Error('boom'); });
assert.throws(() => { JSON.parse('{'); });

// doesNotThrow。
assert.doesNotThrow(() => { return 1 + 1; });

console.log('PASS assert');
