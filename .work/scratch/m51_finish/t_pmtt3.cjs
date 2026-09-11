// 确定性差分：全 worker 侧打印 + 主线程仅 exit 行。
const wt = require('node:worker_threads');
const { Worker, isMainThread, parentPort, threadId, postMessageToThread, workerData } = wt;
function fmt(p, tag) {
  return p.then((v) => `${tag}: resolved(${v})`, (e) => `${tag}: ${e.name}|${e.code}|${e.message}`);
}
if (isMainThread) {
  const log = [];
  process.on('workerMessage', (value, source) => {
    log.push('main got:' + JSON.stringify(value) + ':tid' + source);
    if (value.kind === 'boom') throw new Error('listener boom');
  });
  const w = new Worker(__filename, { workerData: { mode: 'ok' } });
  w.on('message', (m) => {
    log.push('main msg:' + JSON.stringify(m));
    w.terminate();
  });
  w.on('exit', (c) => {
    console.log(log.join('|'));
    console.log('main exit:', c);
  });
} else {
  process.on('workerMessage', (value, source) => {
    if (value.kind === 'slow') {
      const t0 = Date.now();
      while (Date.now() - t0 < 500) {}
      return;
    }
    console.log('worker got:', JSON.stringify(value), 'from tid', source);
  });
  parentPort.on('message', () => {});
  const seq = [];
  seq.push(fmt(postMessageToThread(0, { kind: 'to-main' }), 'worker->main'));
  seq.push(fmt(postMessageToThread(0, { kind: 'boom' }), 'errored'));
  seq.push(fmt(postMessageToThread(0, { kind: 'neg' }, -1), 'neg-timeout'));
  seq.push(fmt(postMessageToThread(threadId, { kind: 'self' }), 'worker-self'));
  seq.push(fmt(postMessageToThread(9999, { kind: 'x' }), 'no-dest'));
  Promise.all(seq).then((lines) => {
    console.log(lines.join('|'));
    parentPort.postMessage('worker-done');
  });
}
