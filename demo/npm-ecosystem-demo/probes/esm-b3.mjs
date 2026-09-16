import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const z = require('zod');
console.log('CREATEREQ', typeof z.string);
