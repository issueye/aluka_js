// JSON.stringify 的 toJSON 协议回归用例（Node 22 对拍）。
//
// 改前：`json_write` 直接按堆变体写值，**从不查询 `toJSON`** —— 于是
// `JSON.stringify({toJSON(){return 1}})` 得 `{}`、`JSON.stringify({d:new Date(0)})`
// 得 `{"d":{}}`（Date 的 ISO 串形态正是靠 `Date.prototype.toJSON` 产出）。
//
// 覆盖规范 `SerializeJSONProperty` 第 2 步：值为对象且 `toJSON` 可调用 → 以属性键
// 为唯一实参调用、用返回值继续序列化；键规则为「根 `""` / 属性名 / 数组下标串」。
//
// **刻意不含循环引用断言**：`JSON.stringify(循环对象)` 在 Node 抛
// `TypeError: Converting circular structure to JSON`，而本实现按既有登记降级为
// `null`（`seen` 守卫，属早已登记的偏离，与本用例主题无关）。
'use strict';

const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

// ---- 基础协议 ----
show('obj-toJSON', () => JSON.stringify({ toJSON() { return 1; } }));
show('nested', () => JSON.stringify({ a: { toJSON() { return 'x'; } } }));
show('arr-elem', () => JSON.stringify([{ toJSON() { return 7; } }]));
show('date-in-obj', () => JSON.stringify({ d: new Date(0) }));
show('date-in-arr', () => JSON.stringify([new Date(0)]));
show('arr-toJSON', () => {
  const a = [1, 2];
  a.toJSON = () => 'arr';
  return JSON.stringify(a);
});
show('inherited', () => JSON.stringify(Object.create({ toJSON() { return 5; } })));

// ---- 键实参（规范：根 ""/属性名/数组下标串）----
show('key-root', () => JSON.stringify({ toJSON(k) { return 'K=' + k; } }));
show('key-prop', () => JSON.stringify({ a: { toJSON(k) { return 'K=' + k; } } }));
show('key-index', () => JSON.stringify([{ toJSON(k) { return 'K=' + k; } }]));
show('key-nested', () => JSON.stringify({ a: [{ toJSON(k) { return 'K=' + k; } }] }));

// ---- this 绑定 ----
show('this-is-obj', () => {
  const o = { v: 42, toJSON() { return this.v; } };
  return JSON.stringify(o);
});

// ---- 返回值语义 ----
show('ret-undefined-root', () => String(JSON.stringify({ toJSON() { return undefined; } })));
show('ret-undefined-prop', () => JSON.stringify({ a: { toJSON() { return undefined; } }, b: 1 }));
show('ret-undefined-arr', () => JSON.stringify([{ toJSON() { return undefined; } }]));
show('ret-null', () => JSON.stringify({ toJSON() { return null; } }));
show('ret-array', () => JSON.stringify({ toJSON() { return [1, 'a']; } }));
show('ret-object', () => JSON.stringify({ toJSON() { return { k: 1 }; } }));
show('ret-becomes-skipped', () => JSON.stringify({ a: { toJSON() { return () => 1; } }, b: 1 }));
show('ret-date', () => JSON.stringify({ d: { toJSON: () => new Date(0) } }));

// ---- 不重复应用（规范：每个属性至多一次）----
show('no-reapply', () => JSON.stringify({ toJSON() { return { toJSON() { return 'inner'; } }; } }));
show('call-count-root', () => {
  let n = 0;
  const o = { toJSON() { n++; return {}; } };
  JSON.stringify(o);
  return n;
});
show('call-count-prop', () => {
  let n = 0;
  const o = { a: { toJSON() { n++; return {}; } } };
  JSON.stringify(o);
  return n;
});

// ---- 非可调用 / 抛错传播 ----
show('noncallable', () => JSON.stringify({ toJSON: 1 }));
show('noncallable-null', () => JSON.stringify({ toJSON: null, a: 1 }));
show('throws', () => {
  try {
    JSON.stringify({ toJSON() { throw new Error('boom'); } });
    return 'no-throw';
  } catch (e) {
    return 'caught:' + e.message;
  }
});

// ---- 回归：既有序列化语义不得改变 ----
show('r-promise', () => JSON.stringify(Promise.resolve(1)));
show('r-map', () => JSON.stringify(new Map()));
show('r-set', () => JSON.stringify(new Set()));
show('r-top-undefined', () => String(JSON.stringify(undefined)));
show('r-top-fn', () => String(JSON.stringify(() => 1)));
show('r-top-symbol', () => String(JSON.stringify(Symbol('s'))));
show('r-arr-undefined', () => JSON.stringify([undefined, () => 1]));
show('r-key-order', () => JSON.stringify({ b: 1, 2: 'two', a: 3, 1: 'one' }));
show('r-symbol-key', () => JSON.stringify({ [Symbol('k')]: 1, a: 2 }));
show('r-nested-mixed', () => JSON.stringify({ a: [1, { b: 2 }], c: 's', d: null, e: true }));
show('r-empty', () => JSON.stringify({}) + '|' + JSON.stringify([]));
