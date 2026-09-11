// 主→worker workerMessage 投递 + worker→主 回程
const { Worker, isMainThread, parentPort, threadId, postMessageToThread } = require('node:worker_threads');
function fmt(p, tag) {
  return p.then((v) => `${tag}: resolved(${v})`, (e) => `${tag}: ${e.name}|${e.code}|${e.message}`);
}
if (isMainThread) {
  process.on('workerMessage', (value, source) => {
    console.log('main got:', JSON.stringify(value), 'from tid', source);
  });
  const w = new Worker(__filename);
  w.on('message', async (tid) => {
    console.log(await fmt(postMessageToThread(tid, { kind: 'hello' }), 'main->worker'));
    console.log(await fmt(postMessageToThread(9999, { kind: 'x' }), 'no-dest'));
    console.log(await fmt(postMessageToThread(0, { kind: 'self' }), 'self'));
    console.log('typeof-ret:', typeof postMessageToThread(tid, { kind: 'second' }));
    w.terminate();
  });
  w.on('exit', (c) => console.log('main exit:', c));
} else {
  process.on('workerMessage', (value, source) => {
    console.log('worker got:', JSON.stringify(value), 'from tid', source);
  });
  parentPort.on('message', () => {});
  parentPort.postMessage(threadId);
}
