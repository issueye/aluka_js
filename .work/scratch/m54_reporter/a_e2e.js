const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('alpha', () => {});
test('beta', () => {});
const s = run();
const c = s.compose(reporters.spec);
c.pipe(process.stdout);
