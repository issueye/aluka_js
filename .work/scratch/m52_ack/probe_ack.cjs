// 取证：worker 自发起断连的 ack 回程可观测面
const cluster = require('node:cluster');
if (cluster.isPrimary) {
  const log = [];
  const w = cluster.fork();
  w.on('online', () => log.push('primary:online'));
  w.on('listening', () => log.push('primary:listening'));
  w.on('message', (m) => {
    log.push('primary:msg:' + JSON.stringify(m));
    if (m.cmd === 'quit') { /* worker 自行断连 */ }
  });
  w.on('disconnect', () => log.push('primary:disconnect'));
  w.on('exit', (code) => {
    log.push('primary:exit:' + code + ':ead:' + w.exitedAfterDisconnect);
    console.log(log.join('|'));
  });
} else {
  const log = [];
  log.push('worker:connected-before:' + process.connected);
  log.push('worker:ead-before:' + cluster.worker.exitedAfterDisconnect);
  const ret = cluster.worker.disconnect();
  log.push('worker:ret-self:' + (ret === cluster.worker));
  log.push('worker:state:' + cluster.worker.state);
  log.push('worker:ead-sync:' + cluster.worker.exitedAfterDisconnect);
  log.push('worker:connected-sync:' + process.connected);
  process.on('disconnect', () => {
    log.push('worker:proc-disc:' + process.connected);
    console.log(log.join('|'));
  });
}
