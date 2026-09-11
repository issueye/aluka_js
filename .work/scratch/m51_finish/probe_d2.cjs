// 探针 D2：成功投递（双向）+ 监听器抛错 + timeout 文本（修正握手）
const { Worker, isMainThread, parentPort, threadId, postMessageToThread } = require('node:worker_threads');
function fmtErr(p, tag) {
  return p.then((v) => `${tag}: resolved(${v})`, (e) => `${tag}: ${e.code}|${e.message}`);
}
if (isMainThread) {
  process.on('workerMessage', (value, source) => {
    console.log('main got:', JSON.stringify(value), 'from tid', source);
    if (value.kind === 'boom') throw new Error('listener boom');
  });
  const w = new Worker(__filename);
  w.on('message', async (m) => {
    console.log(await fmtErr(postMessageToThread(m, { kind: 'hello' }), 'main->worker'));
    console.log(await fmtErr(postMessageToThread(m, { kind: 'boom' }), 'errored'));
    console.log(await fmtErr(postMessageToThread(9999, { kind: 'x' }, 100), 'timeout-text'));
    console.log('ret-of-ok:', await postMessageToThread(m, { kind: 'second' }));
    w.terminate();
  });
  w.on('exit', (c) => console.log('main exit:', c));
} else {
  process.on('workerMessage', (value, source) => {
    if (value.kind === 'boom') throw new Error('worker listener boom');
    console.log('worker got:', JSON.stringify(value), 'from tid', source);
  });
  parentPort.postMessage(threadId);
  fmtErr(postMessageToThread(0, { kind: 'hello-main' }), 'worker->main').then((s) => console.log(s));
}
