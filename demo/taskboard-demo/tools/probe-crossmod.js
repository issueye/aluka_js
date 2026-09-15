'use strict';
// 探针：跨模块抛出的自定义 Error 子类，属性能否穿透模块边界
const { makeAndThrow, PlainThrower } = require('./lib-err');

function probe(label, fn) {
  try {
    fn();
    console.log(label, '=> NO_ERROR');
  } catch (err) {
    console.log(
      label,
      '=> name=' + JSON.stringify(err && err.name),
      'code=' + JSON.stringify(err && err.code),
      'message=' + JSON.stringify(err && err.message),
      'instanceof=' + (err instanceof Error)
    );
  }
}

probe('cross-module new AppError', () => makeAndThrow('boom', 'MYCODE'));
probe('same-module thrower', () => PlainThrower.throwIt());

// 直接跨模块构造（不经过函数调用边界）
const lib = require('./lib-err');
const e = new lib.AppError('direct', 'D1', 418);
console.log(
  'direct construct => name=' + JSON.stringify(e.name),
  'code=' + JSON.stringify(e.code),
  'status=' + JSON.stringify(e.status),
  'message=' + JSON.stringify(e.message)
);
console.log('PROBE6_DONE');
