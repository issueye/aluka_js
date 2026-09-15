// 聚焦探针 A：自定义 Error 子类的实例属性与 name/message 读取
class AppError extends Error {
  constructor(message, code, status) {
    super(message);
    this.name = 'AppError';
    this.code = code;
    this.status = status;
  }
}
class SubError extends AppError {
  constructor(message) {
    super(message, 'SUB', 400);
    this.name = 'SubError';
  }
}

const e = new SubError('boom');
console.log('A.name:', JSON.stringify(e.name));
console.log('A.code:', JSON.stringify(e.code));
console.log('A.status:', JSON.stringify(e.status));
console.log('A.message:', JSON.stringify(e.message));
console.log('A.ownKeys:', JSON.stringify(Object.getOwnPropertyNames(e)));
console.log('A.instanceof:', e instanceof SubError, e instanceof AppError, e instanceof Error);

// 对照：普通类（非 Error 派生）的构造器属性赋值
class Plain {
  constructor() {
    this.name = 'plain';
    this.code = 'PC';
  }
}
const p = new Plain();
console.log('A.plain.name:', JSON.stringify(p.name), JSON.stringify(p.code));

// 对照：直接 new Error 后追加属性
const raw = new Error('raw-message');
raw.code = 'RAW';
console.log('A.raw:', JSON.stringify(raw.name), JSON.stringify(raw.code), JSON.stringify(raw.message));

// 聚焦探针 B：继承 EventEmitter 的类 + 构造器里 this.server 赋值 + 方法内 this.server 读取
const { EventEmitter } = require('node:events');
const http = require('node:http');

class Server extends EventEmitter {
  constructor() {
    super();
    this.server = http.createServer((req, res) => {
      res.end('ok');
    });
  }
  listen(port) {
    return new Promise((resolve) => {
      this.server.listen(port, '127.0.0.1', () => resolve(this.server.address().port > 0));
    });
  }
  close() {
    return new Promise((resolve) => this.server.close(() => resolve('closed')));
  }
}

const s = new Server();
console.log('B.constructed:', typeof s, typeof s.server, typeof s.listen);
s.listen(0)
  .then((ok) => {
    console.log('B.listen:', ok);
    return s.close();
  })
  .then((closed) => {
    console.log('B.close:', closed);
    console.log('PROBE2_DONE');
  })
  .catch((err) => {
    console.log('B.FAILED:', err && err.message);
    console.log('PROBE2_DONE');
  });
