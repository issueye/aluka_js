// eval worker 验证：基本通路 + workerData + __filename
const { Worker } = require('node:worker_threads');
const w = new Worker(`
  const { parentPort, workerData } = require('node:worker_threads');
  parentPort.postMessage('eval-ok:' + workerData.n * 2 + ':fn:' + __filename + ':dir:' + __dirname);
`, { eval: true, workerData: { n: 21 } });
w.on('message', (m) => console.log('E1 message:', m));
w.on('exit', (c) => console.log('E1 exit:', c));
