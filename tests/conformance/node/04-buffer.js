// Node.js 官方测试风格子集：Buffer + TextEncoder。
const assert = require('node:assert');

const b = Buffer.from('hello');
assert.strictEqual(b.length, 5);
assert.strictEqual(b.toString(), 'hello');
assert.strictEqual(b.toString('hex'), '68656c6c6f');
assert.strictEqual(Buffer.from('68656c6c6f', 'hex').toString(), 'hello');
assert.strictEqual(Buffer.from('aGVsbG8=', 'base64').toString(), 'hello');

assert.strictEqual(Buffer.byteLength('héllo'), 6);
assert.ok(Buffer.isBuffer(b));
assert.ok(!Buffer.isBuffer('x'));

const buf = Buffer.alloc(4, 0x41);
assert.strictEqual(buf.toString(), 'AAAA');

// 索引读写。
buf[1] = 0x42;
assert.strictEqual(buf[1], 0x42);

// 只读 length。
buf.length = 100;
assert.strictEqual(buf.length, 4);

// TextEncoder / TextDecoder。
const enc = new TextEncoder().encode('hi');
assert.strictEqual(enc.length, 2);
assert.strictEqual(new TextDecoder().decode(enc), 'hi');

// atob / btoa。
assert.strictEqual(btoa('hello'), 'aGVsbG8=');
assert.strictEqual(atob('aGVsbG8='), 'hello');

// node:buffer 与全局同源。
const { Buffer: NB } = require('node:buffer');
assert.strictEqual(NB, Buffer);

console.log('PASS buffer');
