const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('alpha', () => {});
const s = run();
console.log('stage1 ok');
const c = s.compose(reporters.spec);
console.log('stage2 ok');
c.pipe(process.stdout);
console.log('stage3 ok');
