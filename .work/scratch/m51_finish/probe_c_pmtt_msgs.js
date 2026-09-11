// 探针 C：错误码与错误文本 + workerMessage 双实参 + timeout
const { Worker, isMainThread, threadId, postMessageToThread } = require('node:worker_threads');
function fmtErr(p) {
  return p.then(() => 'resolved', (e) => `${e.name}|${e.code}|${e.message}`);
}
if (isMainThread) {
  fmtErr(postMessageToThread(9999, { a: 1 })).then(console.log);
  fmtErr(postMessageToThread(0, { a: 1 })).then(console.log);
  fmtErr(postMessageToThread(0, { a: 1 }, -1)).then(console.log).catch(() => {});
  const w = new Worker(__filename);
  w.on('exit', (c) => console.log('main exit:', c));
} else {
  process.on('workerMessage', (value, source) => {
    console.log('worker got:', JSON.stringify(value), 'from tid', source);
  });
  fmtErr(postMessageToThread(threadId, { a: 1 })).then(console.log);
  // worker → main（destination 0）：main 有监听器
  fmtErr(postMessageToThread(0, { from: 'worker' })).then((v) => console.log('to-main:', v));
}
