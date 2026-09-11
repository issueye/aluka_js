// 取证：run().compose(reporter) 面
const { test, run } = require('node:test');
const reporters = require('node:test/reporters');
console.log('exports:', Object.keys(reporters).join(','));
console.log('typeof spec:', typeof reporters.spec, '| name:', reporters.spec.name);
const inst = new reporters.spec();
const { Transform } = require('node:stream');
console.log('instanceof Transform:', inst instanceof Transform, '| ctor:', inst.constructor.name);
const s = run();
console.log('stream type:', s.constructor?.name, '| typeof compose:', typeof s.compose);
test('pass-one', () => {});
test('fail-one', () => { throw new Error('boom'); });
s.compose(reporters.spec).pipe(process.stdout);
