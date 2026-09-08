// 测试 for...in 和模块加载
var obj = { a: 1, b: 2, c: 3 };
var keys = [];
for (var k in obj) { keys.push(k); }
console.log('F1 for...in keys:', JSON.stringify(keys));

// 测试 encodings 子模块 - 用 iconv 内部一样的路径
var enc = require('./node_modules/iconv-lite/encodings');
console.log('F2 encodings:', typeof enc, enc ? Object.keys(enc).length : 'null');

// 直接测试 encodings/index.js 的 require 链
var int = require('./node_modules/iconv-lite/encodings/internal');
console.log('F3 internal:', typeof int, int ? Object.keys(int).join(',') : 'null');