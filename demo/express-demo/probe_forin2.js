// 测试 require 模块的 for...in
var int = require('./node_modules/iconv-lite/encodings/internal');
console.log('T1 typeof:', typeof int);
console.log('T2 keys:', Object.keys(int).join(','));
console.log('T3 utf8:', int && int.utf8 ? JSON.stringify(int.utf8) : 'undefined');
console.log('T4 _internal:', typeof (int && int._internal));

// for...in
var keys = [];
for (var k in int) { keys.push(k); }
console.log('T5 forin:', keys.join(','));

// 测试 encodings 整体
var enc = require('./node_modules/iconv-lite/encodings');
console.log('T6 enc typeof:', typeof enc);
console.log('T7 enc keys:', enc ? Object.keys(enc).join(',') : 'null');
console.log('T8 utf8:', enc && enc.utf8 ? JSON.stringify(enc.utf8) : 'undefined');