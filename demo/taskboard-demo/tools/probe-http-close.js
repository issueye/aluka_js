'use strict';
const http = require('node:http');
const t0 = Date.now();
const stamp = function (l) { console.log(l, '+' + (Date.now() - t0)); };
const s = http.createServer((req, res) => res.end('ok'));
s.listen(0, '127.0.0.1', () => {
  stamp('listening');
  s.close(() => { stamp('closed'); });
  stamp('close() called');
});
setTimeout(function () { stamp('wd'); process.exit(0); }, 3000);
