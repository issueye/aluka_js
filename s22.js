const stream = require('stream');
var n = 0;
var log = [];
const w = new stream.Writable({
  highWaterMark: 64 * 1024,
  write(chunk, enc, cb) {
    n++;
    log.push(n);
    setImmediate(cb);
  }
});
w.write('a');
w.write('b');
w.write('c');
w.write('d');
setTimeout(() => { console.log('n:', n, 'log:', JSON.stringify(log)); }, 300);
