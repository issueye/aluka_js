import { createRequire } from 'node:module';
import _ from 'lodash';
const require = createRequire(import.meta.url);
const z = require('zod');
console.log('ESM_OK', _.camelCase('foo bar'), typeof z.string());
