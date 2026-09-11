// 探针 F：eval worker 的 __filename/__dirname + 非法 filename 对比
const { Worker } = require('node:worker_threads');
const w = new Worker(`
  const { parentPort } = require('node:worker_threads');
  parentPort.postMessage(JSON.stringify([typeof __filename, __filename, typeof __dirname, __dirname]));
`, { eval: true });
w.on('message', (m) => console.log('eval fn:', m));
w.on('exit', (c) => {
  console.log('eval exit:', c);
  try { new Worker(42); console.log('no-eval non-string: no throw'); }
  catch (e) { console.log('no-eval non-string threw:', e.code, '|', e.message); }
  try { new Worker('x.js', { eval: 'yes' }); console.log('no throw'); }
  catch (e) { console.log('eval non-bool threw:', e.code, '|', e.message); }
});
