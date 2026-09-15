'use strict';
// 精确刻画：类方法内 `this` 在各类箭头/回调形态下的可见性
function report(label, fn) {
  try {
    console.log(`${label} =>`, JSON.stringify(fn()));
  } catch (err) {
    console.log(`${label} => THROW ${err && err.name}: ${err && err.message}`);
  }
}

class C {
  constructor() {
    this.v = 42;
  }
  // ① 方法体内箭头，直接调用
  arrowDirect() {
    const f = () => this.v;
    return f();
  }
  // ② 方法体内箭头，作为内建方法回调（map）
  arrowInMap() {
    return [1, 2].map(() => this.v)[0];
  }
  // ③ 箭头作为 Promise 执行器
  arrowExecutor() {
    return new Promise((resolve) => resolve(this.v));
  }
  // ④ 箭头在方法体内定义、在 Promise 执行器内调用
  arrowDefinedCalledInExecutor() {
    const f = () => this.v;
    return new Promise((resolve) => resolve(f()));
  }
  // ⑤ 非箭头函数表达式（应无 this）
  functionExpr() {
    const f = function () {
      return this === undefined ? 'undefined-this' : 'has-this';
    };
    return f();
  }
  // ⑥ 显式 self 捕获
  selfCapture() {
    const self = this;
    return new Promise((resolve) => resolve(self.v));
  }
  // ⑦ 箭头作为 setTimeout 回调
  arrowInTimer() {
    return new Promise((resolve) => {
      setTimeout(() => resolve(this.v), 0);
    });
  }
  // ⑧ 调用内建方法时箭头作为实参（Array.from 映射器）
  arrowInFrom() {
    return Array.from({ length: 1 }, () => this.v)[0];
  }
}

const c = new C();
report('① arrowDirect', () => c.arrowDirect());
report('② arrowInMap', () => c.arrowInMap());
report('③ arrowExecutor', () => 'PROMISE(pending)');
report('④ arrowDefinedCalledInExecutor', () => 'PROMISE(pending)');
report('⑤ functionExpr', () => c.functionExpr());
report('⑥ selfCapture', () => 'PROMISE(pending)');
report('⑦ arrowInTimer', () => 'PROMISE(pending)');
report('⑧ arrowInFrom', () => c.arrowInFrom());

Promise.all([
  c.arrowExecutor().then((v) => `③=${v}`, (e) => `③=ERR ${e && e.message}`),
  c.arrowDefinedCalledInExecutor().then((v) => `④=${v}`, (e) => `④=ERR ${e && e.message}`),
  c.selfCapture().then((v) => `⑥=${v}`, (e) => `⑥=ERR ${e && e.message}`),
  c.arrowInTimer().then((v) => `⑦=${v}`, (e) => `⑦=ERR ${e && e.message}`),
]).then((results) => {
  for (const r of results) console.log('ASYNC', r);
  console.log('PROBE4_DONE');
});
