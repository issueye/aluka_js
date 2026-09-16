'use strict';
// zod index.cjs 的 CJS 互操作链逐层定位
function t(label, fn) {
  try {
    console.log(`${label}=${fn()}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

const src = require('../node_modules/zod/v3/external.cjs');
t('external.keys', () => Object.keys(src).length);
t('external.ownNames', () => Object.getOwnPropertyNames(src).length);

// __createBinding 的 create 分支（与 zod index.cjs 一致）
const __createBinding = function (o, m, k, k2) {
  if (k2 === undefined) k2 = k;
  var desc = Object.getOwnPropertyDescriptor(m, k);
  if (!desc || ('get' in desc ? !m.__esModule : desc.writable || desc.configurable)) {
    desc = { enumerable: true, get: function () { return m[k]; } };
  }
  Object.defineProperty(o, k2, desc);
};
const out = {};
let n = 0;
for (const k of Object.keys(src)) {
  __createBinding(out, src, k);
  n++;
}
t('bound.count', () => n);
t('bound.keys', () => Object.keys(out).length);
t('bound.sample', () => JSON.stringify(Object.keys(out).slice(0, 5)));

// 描述符形态抽查
t('desc.sample', () => {
  const k = Object.keys(src)[0];
  const d = Object.getOwnPropertyDescriptor(src, k);
  return JSON.stringify(d && { e: d.enumerable, w: d.writable, c: d.configurable, get: typeof d.get, value: typeof d.value });
});
// __exportStar 使用 for-in（而非 Object.keys）：单独对比两者
const out3 = {};
let forInCount = 0;
for (var p in src) {
  forInCount++;
  if (p !== 'default' && !Object.prototype.hasOwnProperty.call(out3, p)) {
    __createBinding(out3, src, p);
  }
}
t('forIn.count', () => forInCount);
t('exportStar.keys', () => Object.keys(out3).length);
t('exportStar.sample', () => JSON.stringify(Object.keys(out3).slice(0, 5)));

console.log('CREATE_BINDING_PROBE_DONE');
