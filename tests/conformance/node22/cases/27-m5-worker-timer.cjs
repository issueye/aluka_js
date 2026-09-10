// M5 回归用例：定时器等待期间不得饿死 worker 消息投递。
//
// 锁定实测缺陷（见 .work/TODO/20260910/README-m5-review.md §3.1）：
// 主线程存在待触发定时器时，原先事件循环会整段 sleep 到定时器到期，
// 期间不泵 worker 通道 —— 定时器触发前 worker 回包投递不到；若定时器回调里
// terminate()，消息永久丢失。Node 两种情形都立即投递。
//
// 本用例以「消息先于定时器到达」为断言：worker 回包在毫秒级，定时器 1000ms，
// 量级差足以稳定区分（并输出 got 在 tick 之前）。
const { Worker, isMainThread, parentPort } = require('node:worker_threads');

if (!isMainThread) {
  parentPort.on('message', () => parentPort.postMessage('alive'));
} else {
  const w = new Worker(__filename);
  w.on('message', (m) => {
    console.log('tree:', m);
    w.terminate();
  });
  w.on('exit', (code) => console.log('exit:', code));
  w.postMessage({ a: 1 });
  // 待触发定时器：修复前会把上面的 worker 回包推迟到本回调之后
  setTimeout(() => console.log('timer-fired'), 1000);
}
