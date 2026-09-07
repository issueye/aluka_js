// N22-A3：require(esm)——命名/默认导出互操作。
const m = require('./03-esm.mjs');
console.log('result: ' + m.named + '|' + (typeof m.default) + '|' + Object.keys(m).sort().join(','));
