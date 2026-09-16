'use strict';
// 内建构造器静态面 `typeof` 清点（与 Node 对拍；确定性输出）
const CTORS = {
  Object: ['keys', 'values', 'entries', 'fromEntries', 'assign', 'is', 'hasOwn', 'create',
    'getPrototypeOf', 'setPrototypeOf', 'defineProperty', 'defineProperties',
    'getOwnPropertyDescriptor', 'getOwnPropertyNames', 'getOwnPropertySymbols',
    'freeze', 'seal', 'isFrozen', 'isSealed', 'isExtensible', 'preventExtensions'],
  Array: ['isArray', 'from', 'of'],
  String: ['fromCharCode', 'fromCodePoint', 'raw'],
  Number: ['isInteger', 'isFinite', 'isNaN', 'isSafeInteger', 'parseFloat', 'parseInt'],
  Math: ['max', 'min', 'abs', 'floor', 'random'],
  JSON: ['stringify', 'parse'],
  Symbol: ['for', 'keyFor'],
  Reflect: ['get', 'set', 'apply', 'construct', 'ownKeys', 'defineProperty'],
  Promise: ['resolve', 'reject', 'all', 'race', 'allSettled', 'any', 'withResolvers'],
  Date: ['now', 'parse', 'UTC'],
  BigInt: ['asIntN', 'asUintN'],
  Error: ['captureStackTrace', 'stackTraceLimit'],
};
for (const ctor of Object.keys(CTORS)) {
  const target = globalThis[ctor];
  if (!target) {
    console.log(`${ctor}: MISSING_CTOR`);
    continue;
  }
  for (const m of CTORS[ctor]) {
    console.log(`${ctor}.${m}=${typeof target[m]}`);
  }
}
console.log('STATIC_TYPEOF_DONE');
