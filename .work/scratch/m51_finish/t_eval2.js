const { Worker } = require('node:worker_threads');
const w2 = new Worker(`throw new TypeError('boom-eval');`, { eval: true });
w2.on('error', (e) => console.log('E2 error:', e.constructor.name, '|', e.name, '|', e.message));
w2.on('exit', (c2) => {
  console.log('E2 exit:', c2);
  try {
    new Worker(42, { eval: true });
    console.log('E3: no throw');
  } catch (e) {
    console.log('E3 threw:', e.code, '|', e.message);
  }
  try {
    new Worker(42);
    console.log('E3b: no throw');
  } catch (e) {
    console.log('E3b threw:', e.code, '|', e.message);
  }
});
