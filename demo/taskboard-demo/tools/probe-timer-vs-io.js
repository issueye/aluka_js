'use strict';
const http = require('node:http');
const t0 = Date.now();
const stamp = function (label) { console.log(label, 'at +' + (Date.now() - t0) + 'ms'); };
const s = http.createServer((req, res) => res.end('ok'));
setTimeout(function () { stamp('timer(3000)'); }, 3000);
setTimeout(function () { stamp('timer(300)'); }, 300);
s.listen(0, '127.0.0.1', () => { stamp('listening-cb'); s.close(); });
stamp('registered');
