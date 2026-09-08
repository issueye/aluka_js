const stream = require('stream');
console.log('A');
const w = new stream.Writable({ highWaterMark: 16, write(chunk, enc, cb) { setImmediate(cb); } });
console.log('B');
w.write('chunk-0');
console.log('C');
setTimeout(() => { console.log('timer'); }, 100);
