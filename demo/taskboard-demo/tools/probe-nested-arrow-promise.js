'use strict';
const { S } = require('./hmod5');
const s = new S();
const wd = setTimeout(function () { console.log('WATCHDOG'); process.exit(3); }, 3000);
s.close().then(function (v) { console.log('THEN-nested:', v); return s.closeSimple(); }).then(function (v) { console.log('THEN-simple:', v); clearTimeout(wd); process.exit(0); }).catch(function (e) { console.log('FAILED:', e && e.message); process.exit(1); });
