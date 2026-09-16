export const a = 42;
console.log('SIDE-B');
try { const z = require('zod'); console.log('ZOD', typeof z.string); } catch (e) { console.log('ERR', e.name, e.message); }
