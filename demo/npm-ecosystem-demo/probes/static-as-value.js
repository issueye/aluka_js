'use strict';
// 「内建静态方法作为**值**调用」探针：提取为变量后调用（不是 obj.m(...) 形态）。
// 覆盖 Object/Array/String/Number/Math/JSON/Symbol/Reflect 的常用静态面。
function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}`);
  }
}

t('Object.keys', () => { const f = Object.keys; return f({ a: 1 }); });
t('Object.values', () => { const f = Object.values; return f({ a: 1 }); });
t('Object.entries', () => { const f = Object.entries; return f({ a: 1 }); });
t('Object.fromEntries', () => { const f = Object.fromEntries; return f([['a', 1]]); });
t('Object.assign', () => { const f = Object.assign; return f({}, { a: 1 }); });
t('Object.is', () => { const f = Object.is; return f(NaN, NaN); });
t('Object.hasOwn', () => { const f = Object.hasOwn; return f({ a: 1 }, 'a'); });
t('Object.getOwnPropertyNames', () => { const f = Object.getOwnPropertyNames; return f({ a: 1 }); });
t('Object.getPrototypeOf', () => { const f = Object.getPrototypeOf; return typeof f({}); });
t('Object.create', () => { const f = Object.create; return typeof f(null); });
t('Object.freeze', () => { const f = Object.freeze; return typeof f({}); });
t('Object.isFrozen', () => { const f = Object.isFrozen; return f(Object.freeze({})); });
t('Object.seal', () => { const f = Object.seal; return typeof f({}); });
t('Object.isExtensible', () => { const f = Object.isExtensible; return f({}); });
t('Object.defineProperty', () => {
  const f = Object.defineProperty; const o = {}; f(o, 'x', { value: 1 }); return o.x;
});
t('Array.isArray', () => { const f = Array.isArray; return f([1]); });
t('Array.from', () => { const f = Array.from; return f('ab'); });
t('Array.of', () => { const f = Array.of; return f(1, 2); });
t('String.fromCharCode', () => { const f = String.fromCharCode; return f(65); });
t('String.fromCodePoint', () => { const f = String.fromCodePoint; return f(97); });
t('Number.isInteger', () => { const f = Number.isInteger; return f(3); });
t('Number.isNaN', () => { const f = Number.isNaN; return f(NaN); });
t('Number.parseFloat', () => { const f = Number.parseFloat; return f('3.5'); });
t('Number.parseInt', () => { const f = Number.parseInt; return f('42px'); });
t('Math.max', () => { const f = Math.max; return f(1, 2); });
t('JSON.stringify', () => { const f = JSON.stringify; return f({ a: 1 }); });
t('JSON.parse', () => { const f = JSON.parse; return f('{"a":1}'); });
t('Symbol.for', () => { const f = Symbol.for; return typeof f('x'); });
t('Reflect.get', () => { const f = Reflect.get; return f({ a: 1 }, 'a'); });
console.log('STATIC_AS_VALUE_DONE');
