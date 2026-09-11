// 探针 A：postMessageToThread 主线程→worker 基础通路 + 返回值
const { Worker, isMainThread, parentPort, postMessageToThread } = require('node:worker_threads');
if (isMainThread) {
  const w = new Worker(__filename);
  w.on('message', (m) => console.log('main got:', JSON.stringify(m)));
  w.on('exit', (c) => console.log('main exit:', c));
  console.log('ret:', typeof postMessageToThread(w.threadId, { hello: 'world' }));
} else {
  parentPort.on('message', (m) => {
    console.log('worker got:', JSON.stringify(m));
    parentPort.postMessage('done');
  });
}
