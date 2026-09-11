// 探针 E：eval worker 基础 + workerData + 异常 + 非法参数
const { Worker } = require('node:worker_threads');
// E1: 基础 + workerData + require + parentPort
const w = new Worker(`
  const { parentPort, workerData } = require('node:worker_threads');
  parentPort.postMessage('eval-ok:' + workerData.n * 2);
`, { eval: true, workerData: { n: 21 } });
w.on('message', (m) => console.log('E1 message:', m));
w.on('exit', (c) => {
  console.log('E1 exit:', c);
  // E2: 未捕获异常
  const w2 = new Worker(`throw new TypeError('boom-eval');`, { eval: true });
  w2.on('error', (e) => console.log('E2 error:', e.constructor.name, '|', e.message));
  w2.on('exit', (c2) => {
    console.log('E2 exit:', c2);
    // E3: 非字符串源码
    try {
      new Worker(42, { eval: true });
      console.log('E3: no throw');
    } catch (e) {
      console.log('E3 threw:', e.code, '|', e.message);
    }
    // E4: eval 语法错误
    const w4 = new Worker(`syntax (( error`, { eval: true });
    w4.on('error', (e) => console.log('E4 error:', e.constructor.name, '|', e.message));
    w4.on('exit', (c4) => console.log('E4 exit:', c4));
  });
});
