// M5 差分用例：worker_threads eval worker（`new Worker(src, { eval: true })`）。
// Node 22.23.1 口径：eval 源码在 worker 线程现场执行，require/parentPort/
// workerData 可用；`__filename === '[worker eval]'`、`__dirname === '.'`；
// 未捕获 TypeError → 主线程 'error' 收到 Error 对象（name/message）+ 'exit'(1)；
// 语法错误 → SyntaxError + exit 1（消息文本为引擎解析器自有，不做逐字对拍）；
// 非字符串 filename + eval:true → 同步抛 ERR_INVALID_ARG_VALUE；无 eval →
// 同步抛 ERR_INVALID_ARG_TYPE。
const { Worker } = require('node:worker_threads');

// E1：基本通路 + workerData + __filename/__dirname
const w = new Worker(
  `
  const { parentPort, workerData } = require('node:worker_threads');
  parentPort.postMessage('eval-ok:' + workerData.n * 2 + ':fn:' + __filename + ':dir:' + __dirname);
`, { eval: true, workerData: { n: 21 } });
w.on('message', (m) => console.log('E1 message:', m));
w.on('exit', (c) => {
  console.log('E1 exit:', c);
  // E2：未捕获异常 → 'error' 收到 Error 对象（name/message；constructor.name
  // 为登记偏离，不打印）
  const w2 = new Worker(`throw new TypeError('boom-eval');`, { eval: true });
  w2.on('error', (e) => {
    console.log('E2 error:', e instanceof Error, e.name, '|', e.message);
  });
  w2.on('exit', (c2) => {
    console.log('E2 exit:', c2);
    // E3：非字符串源码的同步校验
    try {
      new Worker(42, { eval: true });
      console.log('E3: no throw');
    } catch (e) {
      console.log('E3 threw:', e.code, '|', e.message);
    }
    try {
      new Worker(42);
      console.log('E3b: no throw');
    } catch (e) {
      console.log('E3b threw:', e.code, '|', e.message);
    }
    // E4：语法错误 → SyntaxError + exit 1（只对拍 name 与退出码）
    const w4 = new Worker(`syntax (( error`, { eval: true });
    w4.on('error', (e) => console.log('E4 error:', e instanceof Error, e.name));
    w4.on('exit', (c4) => console.log('E4 exit:', c4));
  });
});
