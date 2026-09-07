// Node.js 官方测试风格子集：events + EventTarget。
const { EventEmitter } = require('node:events');
const assert = require('node:assert');

const emitter = new EventEmitter();
let count = 0;

emitter.on('ping', (x) => { count += x; });
emitter.once('once', () => { count += 10; });

emitter.emit('ping', 1);
emitter.emit('ping', 2);
emitter.emit('once');
emitter.emit('once'); // once 只触发一次

assert.strictEqual(count, 13);
assert.strictEqual(emitter.listenerCount('ping'), 1);

// 链式与 removeListener。
const f = () => {};
emitter.on('x', f);
emitter.removeListener('x', f);
assert.strictEqual(emitter.listenerCount('x'), 0);

// 全局 EventTarget（WHATWG）。
const target = new EventTarget();
let ev = null;
target.addEventListener('go', (e) => { ev = e.type; });
target.dispatchEvent(new Event('go'));
assert.strictEqual(ev, 'go');

// CustomEvent detail。
const ce = new CustomEvent('data', { detail: 42 });
assert.strictEqual(ce.detail, 42);

console.log('PASS events');
