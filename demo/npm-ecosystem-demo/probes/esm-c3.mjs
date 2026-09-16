console.log('SIDE-A');
try { const p = require('zod'); console.log('ZOD', typeof p.string); } catch (e) { console.log('ERR', e.name, e.message); }
