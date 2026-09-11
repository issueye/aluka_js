// worker→worker 中转 + 真实超时到期（目标忙 500ms，timeout 50ms）
const wt = require('node:worker_threads');
const { Worker, isMainThread, parentPort, postMessageToThread, workerData } = wt;
function fmt(p, tag) {
  return p.then((v) => `${tag}: resolved(${v})`, (e) => `${tag}: ${e.name}|${e.code}|${e.message}`);
}
if (isMainThread) {
  const log = [];
  const wb = new Worker(__filename, { workerData: { mode: 'B' } });
  const wa = new Worker(__filename, { workerData: { mode: 'A', b_tid: wb.threadId } });
  wa.on('message', (m) => {
    log.push('A said:' + JSON.stringify(m));
    wa.terminate();
    wb.terminate();
  });
  wa.on('exit', (c) => {
    console.log(log.join('|'));
    console.log('main exit:', c);
  });
} else if (workerData.mode === 'A') {
  process.on('workerMessage', () => {});
  parentPort.on('message', () => {});
  const bTid = workerData.b_tid;
  fmt(postMessageToThread(bTid, { kind: 'hello-b' }), 'a->b').then((s) => {
    console.log(s);
    return fmt(postMessageToThread(bTid, { kind: 'slow' }, 50), 'timeout');
  }).then((s) => {
    console.log(s);
    parentPort.postMessage('done');
  });
} else {
  process.on('workerMessage', (value) => {
    if (value.kind === 'slow') {
      const t0 = Date.now();
      while (Date.now() - t0 < 500) {}
      return;
    }
    console.log('B got:', JSON.stringify(value));
  });
  parentPort.on('message', () => {});
}
