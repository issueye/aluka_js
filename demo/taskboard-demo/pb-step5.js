'use strict';
require('./src/config');
require('./src/logger');
require('./src/errors');
require('./src/store');
require('./src/service');
const { ConflictError } = require('./src/errors');
try { const e = new ConflictError('step-msg'); console.log('step5 OK:', JSON.stringify(e.name), JSON.stringify(e.code)); } catch (err) { console.log('step5 THROWN:', typeof err, JSON.stringify(err && err.name)); }
console.log('STEP5 DONE');
