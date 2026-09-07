// N22-A4：process.nextTick 优先于 Promise 微任务，再 timer。
let order = [];
process.nextTick(() => order.push('tick'));
Promise.resolve().then(() => order.push('promise'));
queueMicrotask(() => order.push('micro'));
setTimeout(() => order.push('timer'));
setTimeout(() => console.log('result: ' + order.join(',')), 20);
