// 探针 B：握手后 postMessageToThread 成功通路 + 返回值形态
const { Worker, isMainThread, parentPort, threadId, postMessageToThread } = require('node:worker_threads');
if (isMainThread) {
  const w = new Worker(__filename);
  w.on('message', (m) => {
    console.log('main got:', JSON.stringify(m));
    const ret = postMessageToThread(m.tid, { hello: 'world', n: 42 });
    console.log('ret keys:', ret === undefined ? 'undefined' : JSON.stringify(Object.keys(ret)));
  });
  w.on('exit', (c) => console.log('main exit:', c));
} else {
  parentPort.on('message', (m) => {
    console.log('worker got:', JSON.stringify(m));
    parentPort.postMessage({ tid: threadId, reply: 'done' });
  });
  parentPort.postMessage({ tid: threadId, hello: 'init' });
}
