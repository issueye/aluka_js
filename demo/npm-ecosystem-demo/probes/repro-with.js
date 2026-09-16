// 轮 100 复现之 with 语句：lodash `_.template` 经 `new Function` 编译出
// `with(obj){...}` 非严格体（aluka 解析器当前不支持 with，整文件拒绝）。
const o = { v: 5 };
let r;
with (o) { r = v; }
console.log('withStatement=' + r);
// with 作用域内的赋值回写与遮蔽
with (o) { v = 9; }
console.log('withWriteback=' + o.v);
const shadow = 'outer';
console.log('withShadow=' + (function () { const shadow = 'inner'; with ({}) { return shadow; } })());
console.log('WITH_DONE');
