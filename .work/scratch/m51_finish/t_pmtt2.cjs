// worker→主 投递 + 监听器抛错 ERRORED + timeout 校验（workerData 分支）
const wt = require('node:worker_threads');
const { Worker, isMainThread, parentPort, threadId, postMessageToThread, workerData } = wt;
function fmt(p, tag) {
  return p.then((v) => `${tag}: resolved(${v})`, (e) => `${tag}: ${e.name}|${e.code}|${e.message}`);
}
if (isMainThread) {
  process.on('workerMessage', (value, source) => {
    console.log('main got:', JSON.stringify(value), 'from tid', source);
    if (value.kind === 'boom') throw new Error('listener boom');
  });
  const w = new Worker(__filename, { workerData: { mode: 'ok' } });
  w.on('exit', (c) => console.log('main exit:', c));
} else if (workerData.mode === 'ok') {
  process.on('workerMessage', (value) => {
    console.log('worker got:', JSON.stringify(value));
  });
  parentPort.on('message', () => {});
  fmt(postMessageToThread(0, { kind: 'to-main' }), 'worker->main').then((s) => console.log(s));
  fmt(postMessageToThread(0, { kind: 'boom' }), 'errored').then((s) => {
    console.log(s);
    return fmt(postMessageToThread(0, { kind: 'neg' }, -1), 'neg-timeout');
  }).then((s) => {
    console.log(s);
    return fmt(postMessageToThread(threadId, { kind: 'x' }), 'worker-self');
  }).then((s) => {
    console.log(s);
    parentPort.postMessage('worker-done');
  });
}
