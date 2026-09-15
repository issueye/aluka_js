'use strict';
const t0 = Date.now();
const stamp = function (label) { console.log(label, 'at +' + (Date.now() - t0) + 'ms'); };
setTimeout(function () { stamp('timer(3000)'); }, 3000);
setTimeout(function () { stamp('timer(300)'); }, 300);
stamp('registered');
