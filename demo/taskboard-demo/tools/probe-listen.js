'use strict';
// 探针：EventEmitter 子类在 listen 回调里 emit（无监听器）+ resolve 顺序
const { EventEmitter } = require('node:events');
const http = require('node:http');

class S extends EventEmitter {
  constructor() {
    super();
    this.server = http.createServer((req, res) => res.end('ok'));
  }
  listen(port, host) {
    return new Promise((resolve) => {
      this.server.listen(port, host, () => {
        const address = this.server.address();
        console.log('  [cb] address() =>', typeof address, address && typeof address.port);
        const emitted = this.emit('listening', address);
        console.log('  [cb] emit result =>', emitted);
        resolve(address);
      });
    });
  }
  close() {
    return new Promise((resolve) => this.server.close(() => resolve('closed')));
  }
}

const s = new S();
console.log('hasListeners:', s.listenerCount ? s.listenerCount('listening') : 'n/a');
s.listen(0, '127.0.0.1')
  .then((address) => {
    console.log('listen resolved:', address.port > 0);
    return s.close();
  })
  .then((closed) => {
    console.log('close:', closed);
    console.log('PROBE5_DONE');
  })
  .catch((err) => {
    console.log('PROBE5_FAILED:', err && err.message);
    console.log('PROBE5_DONE');
  });
