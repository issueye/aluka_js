const reporters = require('node:test/reporters');
const { Transform } = require('node:stream');
const { run } = require('node:test');
for (const k of ['dot','junit','spec','tap','lcov']) {
  const R = reporters[k];
  let inst;
  try { inst = new R(); } catch (e) { inst = null; }
  console.log(`${k}: typeof=${typeof R} name=${R.name} instCtor=${inst ? inst.constructor.name : '-'} isT=${inst instanceof Transform} wObjMode=${inst && inst.writableObjectMode}`);
}
const s = run();
const c = s.compose(reporters.spec);
console.log('composed ctor:', c && c.constructor && c.constructor.name, '| typeof pipe:', typeof (c && c.pipe));
