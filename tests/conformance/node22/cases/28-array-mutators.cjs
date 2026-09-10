// 数组变异方法回归用例：pop / shift / unshift 的返回值与数组改写。
//
// 背景：interpreter 的数组内联分派（`match method_name` 于 Array 分支）此前
// **缺 pop/shift/unshift 三个分支**，调用落到通用路径返回 `undefined` 且
// **不改写数组**；surface.rs 的注册表路径缺 shift/unshift。该缺陷此前只被登记在
// `cases/gen/deviations/`（隔离区，不参与门禁），故 864 例全绿也未能发现——
// 本用例把它移入门禁语料。
const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

show('pop', () => {
  const a = [1, 2, 3];
  const r = a.pop();
  return r + '|' + JSON.stringify(a);
});
show('pop-empty', () => {
  const a = [];
  const r = a.pop();
  return String(r) + '|' + JSON.stringify(a);
});
show('shift', () => {
  const a = [1, 2, 3];
  const r = a.shift();
  return r + '|' + JSON.stringify(a);
});
show('shift-empty', () => {
  const a = [];
  const r = a.shift();
  return String(r) + '|' + JSON.stringify(a);
});
show('unshift', () => {
  const a = [3];
  const r = a.unshift(1, 2);
  return r + '|' + JSON.stringify(a);
});
show('unshift-empty', () => {
  const a = [];
  const r = a.unshift('x');
  return r + '|' + JSON.stringify(a);
});
show('push', () => {
  const a = [1];
  const r = a.push(2, 3);
  return r + '|' + JSON.stringify(a);
});
show('splice', () => {
  const a = [1, 2, 3];
  const r = a.splice(1, 1);
  return JSON.stringify(r) + '|' + JSON.stringify(a);
});
// 求值顺序：实参从左到右（JS 规范），故输出与数组终态均确定
show('pop-all', () => {
  const a = [1, 2];
  const out = [];
  out.push(a.pop(), a.pop(), a.pop());
  return JSON.stringify(out) + '|' + JSON.stringify(a);
});
show('mixed', () => {
  const a = [1, 2];
  a.unshift(0);
  a.push(3);
  a.shift();
  a.pop();
  return JSON.stringify(a);
});
show('length-after', () => {
  const a = [1, 2, 3];
  a.pop();
  return a.length;
});
show('str-elements', () => {
  const a = ['a', 'b'];
  const r = a.shift();
  a.unshift('z');
  return r + '|' + JSON.stringify(a);
});
