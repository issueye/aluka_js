const { mock } = require('node:test');
// mock.fn 基本
const fn = mock.fn((a, b) => a + b);
console.log('m1:', typeof fn, typeof fn.mock, typeof fn.mock.calls);
console.log('m2:', fn(1, 2), fn(3, 4));
console.log('m3:', fn.mock.calls.length, fn.mock.calls[0].arguments.length);
console.log('m4:', fn.mock.calls[0].arguments[0], fn.mock.calls[1].arguments[1]);
console.log('m5:', fn.mock.calls[1].result);
console.log('m6:', fn.mock.calls[0].error === undefined);
const fn2 = mock.fn();
console.log('m7:', fn2.mock.calls.length);
// calls 元素属性
const fn3 = mock.fn((a) => a * 2);
fn3(5);
const c = fn3.mock.calls[0];
console.log('m8:', Object.keys(c).sort().join(','));
console.log('m9:', c.arguments.length, c.result);
// mock.method
const obj = { add(a, b) { return a + b; } };
const spy = mock.method(obj, 'add');
console.log('m10:', obj.add(2, 3));
console.log('m11:', spy.mock.calls.length, spy.mock.calls[0].arguments.join(','));
console.log('m12:', typeof obj.add.mock);
obj.add.mock.restore();
console.log('m13:', obj.add(5, 6), typeof obj.add.mock);
// mock.fn 的 this 透传
const fn4 = mock.fn(function (x) { return this.v * x; });
console.log('m14:', fn4.call({ v: 10 }, 3));
// restoreAll
const fn6 = mock.fn(() => 2);
fn6();
mock.restoreAll();
console.log('m16:', fn6.mock.calls.length);
// 抛错记录
const fn7 = mock.fn(() => { throw new Error('boom'); });
try { fn7(); } catch {}
console.log('m17:', fn7.mock.calls[0].error !== undefined);
