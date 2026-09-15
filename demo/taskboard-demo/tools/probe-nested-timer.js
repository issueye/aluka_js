'use strict';
const t0 = Date.now();
const stamp = function (l) { console.log(l, '+' + (Date.now() - t0)); };
setTimeout(function () {
  stamp('outer-timer');
  setTimeout(function () { stamp('inner-timer'); }, 0);
  setImmediate(function () { stamp('inner-immediate'); });
}, 0);
setTimeout(function () { stamp('wd'); process.exit(0); }, 800);
