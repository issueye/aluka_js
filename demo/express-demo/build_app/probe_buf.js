// 测 Buffer 静态方法
console.log('B1 byteLength:', Buffer.byteLength('{"ok":true}', 'utf8'));
console.log('B2 from type:', typeof Buffer.from);
var b = Buffer.from('{"ok":true}', 'utf8');
console.log('B3 from result type:', typeof b, b && b.length);
var s = '{"ok":true}';
console.log('B4 typeof s:', typeof s, 'len:', s.length);
var n = 12;
console.log('B5 n.toString(16):', n.toString(16));
var u;
try { console.log('B6 undef toString:', u.toString(16)); } catch (e) { console.log('B6 CAUGHT:', e.message); }