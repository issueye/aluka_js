const { test, run } = require('node:test');
const reporters = require('node:test/reporters');
test('pass-one', () => {});
test('fail-one', () => { throw new Error('boom'); });
run().compose(reporters.tap).pipe(process.stdout);
