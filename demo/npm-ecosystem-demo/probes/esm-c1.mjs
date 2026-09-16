import { createRequire } from 'node:module';
console.log('URL', typeof import.meta.url);
const require = createRequire(import.meta.url);
console.log('REQ', typeof require);
