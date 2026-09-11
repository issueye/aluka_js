const { run, test } = require('node:test');
const reporters = require('node:test/reporters');
test('a', () => {});
const inst = new reporters.spec();
console.log('kind-of-inst:', require('util').types ? 'skip' : 'skip');
const s = run();
try {
  const c = s.compose(inst);
  console.log('instance-path ok:', typeof c.pipe);
} catch (e) { console.log('instance-path ERR:', e.message); }
