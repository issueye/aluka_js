'use strict';
// 对照脚本（与 pb-c.js 同模块集合、仅 require 顺序不同 → 通过）
require('./src/config');
require('./src/logger');
require('./src/errors');
require('./src/service');
require('./src/store');
const { ConflictError } = require('./src/errors');
try {
  const e = new ConflictError('pb-msg');
  console.log('pb-h.js ok:', JSON.stringify(e.name), JSON.stringify(e.code));
} catch (err) {
  console.log('pb-h.js thrown:', err && err.name, '|', err && err.message);
}
console.log('pb-h.js DONE');
