// M5 差分用例：worker_threads.postMessageToThread 基础通路与错误面。
// Node 22.23.1 口径：投递走 process.on('workerMessage')（不经 parentPort），
// 返回 Promise；同线程 → ERR_WORKER_MESSAGING_SAME_THREAD；无目标/无监听器 →
// ERR_WORKER_MESSAGING_FAILED；监听器抛错 → ERR_WORKER_MESSAGING_ERRORED；
// timeout < 0 → RangeError ERR_OUT_OF_RANGE。
// 输出确定性：worker 侧逐行直打；主线程只在 exit 时汇总打印（先 log 后 exit）。
const wt = require('node:worker_threads');
const {
  Worker,
  isMainThread,
  parentPort,
  threadId,
  postMessageToThread,
} = wt;

function fmt(p, tag) {
  return p.then(
    (v) => `${tag}: resolved(${v})`,
    (e) => `${tag}: ${e.name}|${e.code}|${e.message}`,
  );
}

if (isMainThread) {
  const log = [];
  process.on('workerMessage', (value, source) => {
    log.push('main got:' + JSON.stringify(value) + ':tid' + source);
    if (value.kind === 'boom') throw new Error('listener boom');
  });
  const w = new Worker(__filename);
  w.on('message', (m) => {
    if (m === 'ready') {
      postMessageToThread(w.threadId, { kind: 'hello' });
    } else if (m === 'worker-done') {
      w.terminate();
    }
  });
  w.on('exit', (c) => {
    console.log(log.join('|'));
    console.log('main exit:', c);
  });
} else {
  let started = false;
  process.on('workerMessage', (value, source) => {
    console.log('main->worker got:', JSON.stringify(value), 'from tid', source);
    if (started) return;
    started = true;
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
  });
  parentPort.on('message', () => {});
  parentPort.postMessage('ready');
}
