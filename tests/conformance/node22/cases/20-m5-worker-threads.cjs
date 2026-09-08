// M5 差分用例：worker_threads 真实跨物理线程。
// 模式：单文件自引用（worker 重新运行本文件，isMainThread 分支区分角色）。
const {
  Worker,
  isMainThread,
  parentPort,
  workerData,
  threadId,
} = require('node:worker_threads');

if (isMainThread) {
  // worker A：回发计算结果后由主线程主动 terminate（exit 1，时序确定）
  const w = new Worker(__filename, { workerData: { n: 20, label: 'sq' } });
  const got = [];
  w.on('message', (m) => {
    got.push(m);
    w.terminate();
  });
  w.on('exit', (code) => {
    console.log('main exit:', code, got.join('|'));
    // worker B：首个 tick 到达后再 terminate（确保已启动，exit 1）
    const w2 = new Worker(__filename, { workerData: { label: 'term' } });
    w2.on('message', () => w2.terminate());
    w2.on('exit', (code2) => {
      console.log('main exit2:', code2);
    });
  });
  w.postMessage('start');
} else if (workerData.label === 'sq') {
  parentPort.on('message', (m) => {
    parentPort.postMessage(`sq:${workerData.n * workerData.n}:${m}:t${threadId}`);
  });
} else {
  setInterval(() => parentPort.postMessage('tick'), 50);
}
