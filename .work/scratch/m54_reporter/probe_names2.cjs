const reporters = require('node:test/reporters');
const { Transform } = require('node:stream');
const { run } = require('node:test');
for (const k of ['dot','tap','junit']) {
  let errNew = '-', errCall = '-', callInst = '-';
  try { new reporters[k](); } catch (e) { errNew = e.message.slice(0, 60); }
  try { const i = reporters[k](); callInst = i ? (i.constructor.name + '|' + (i instanceof Transform)) : 'null'; } catch (e) { errCall = e.message.slice(0, 60); }
  console.log(`${k}: newErr=${errNew} callErr=${errCall} call=${callInst}`);
}
console.log('lcov keys:', Object.keys(reporters.lcov).join(','), '| vals:', Object.values(reporters.lcov).map((v) => typeof v).join(','));
const s = run();
console.log('compose-tap-ctor:', s.compose(reporters.tap).constructor.name);
