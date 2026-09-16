import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
try { const p = require('./dep.cjs'); console.log('REL', typeof p); } catch (e) { console.log('ERR', e.message); }
