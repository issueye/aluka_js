// N22-C4：navigator 存在性 + BroadcastChannel 广播。
// 注意：BroadcastChannel 持有事件循环句柄，结束后必须 close()
// （Node 22 语义），否则进程不退出。
const out = [];
out.push('nav:' + typeof navigator + '|' + (navigator.hardwareConcurrency > 0) + '|' + (typeof navigator.platform === 'string'));
const a = new BroadcastChannel('chat');
const b = new BroadcastChannel('chat');
a.addEventListener('message', e => out.push('a:' + e.data));
b.addEventListener('message', e => out.push('b:' + e.data));
setTimeout(() => a.postMessage('hello'), 5);
setTimeout(() => {
  console.log('result: ' + out.join(';'));
  a.close();
  b.close();
}, 30);
