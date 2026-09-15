'use strict';
require('./src/config');
require('./src/logger');
require('./src/errors');
require('./src/store');
require('./src/service');
const { ConflictError } = require('./src/errors');
try { const e = new ConflictError('pb-msg'); console.log('pb-c.js ok:', JSON.stringify(e.name), JSON.stringify(e.code)); } catch (err) { console.log('pb-c.js thrown:', err && err.name, '|', err && err.message); }
console.log('pb-c.js DONE');
