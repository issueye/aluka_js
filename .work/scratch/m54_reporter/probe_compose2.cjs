const { test, run } = require('node:test');
const reporters = require('node:test/reporters');
test('pass-one', () => {});
test('fail-one', () => { throw new Error('boom'); });
const s = run();
console.error('ctor-name:' + reporters.spec.name + '|instanceof:' + (new reporters.spec() instanceof require('node:stream').Transform));
s.compose(reporters.spec).pipe(process.stdout);
