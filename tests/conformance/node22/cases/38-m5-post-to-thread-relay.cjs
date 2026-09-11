// M5 差分用例：worker_threads.postMessageToThread 跨 worker 中转与超时。
// Node 22.23.1 口径：worker→worker 投递经主线程中转；目标线程忙导致应答
// 晚于 timeout → ERR_WORKER_MESSAGING_TIMEOUT
// （`Sending a message to another thread timed out`）。
// 输出确定性：worker 侧逐行直打；主线程只在 exit 时汇总打印。
const wt = require('node:worker_threads');
const {
  Worker,
  isMainThread,
  parentPort,
  postMessageToThread,
  workerData,
} = wt;

function fmt(p, tag) {
  return p.then(
    (v) => `${tag}: resolved(${v})`,
    (e) => `${tag}: ${e.name}|${e.code}|${e.message}`,
  );
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
  fmt(postMessageToThread(bTid, { kind: 'hello-b' }), 'a->b')
    .then((s) => {
      console.log(s);
      // B 侧忙等 500ms：50ms 超时先到期 → ERR_WORKER_MESSAGING_TIMEOUT
      return fmt(postMessageToThread(bTid, { kind: 'slow' }, 50), 'timeout');
    })
    .then((s) => {
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
