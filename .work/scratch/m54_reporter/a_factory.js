const reporters = require('node:test/reporters');
try {
  const i = reporters.spec();
  console.log('factory-call ok, kind:', i._reporterKind);
} catch (e) { console.log('factory-call err:', e.message); }
try {
  const i2 = new reporters.spec();
  console.log('new ok, kind:', i2._reporterKind);
} catch (e) { console.log('new err:', e.message); }
