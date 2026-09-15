'use strict';
// 最小复现：类方法内 this 绑定（Promise 执行器箭头 / 回调 / then 链）
class C {
  constructor() {
    this.v = 42;
  }
  m1() {
    return new Promise((resolve) => resolve(this.v));
  }
  m2() {
    return Promise.resolve(this.v);
  }
  m3() {
    const self = this;
    return new Promise((resolve) => resolve(self.v));
  }
  m4() {
    return new Promise((resolve) => resolve(null)).then(() => this.v);
  }
  m5(cb) {
    // 回调不是箭头（由调用方决定 this）
    return cb(this.v);
  }
  m6() {
    const inner = () => this.v;
    return inner();
  }
}

const c = new C();
console.log('m1 (Promise executor arrow):', c.m1() instanceof Promise ? 'PROMISE' : '?');
c.m1()
  .then((v) => {
    console.log('m1 value:', v);
    return c.m2();
  })
  .then((v) => {
    console.log('m2 value:', v);
    return c.m3();
  })
  .then((v) => {
    console.log('m3 value:', v);
    return c.m4();
  })
  .then((v) => {
    console.log('m4 value:', v);
    console.log('m5 value:', c.m5((x) => x));
    console.log('m6 value:', c.m6());
    console.log('PROBE3_DONE');
  })
  .catch((err) => {
    console.log('PROBE3_FAILED:', err && err.message);
    console.log('PROBE3_DONE');
  });
