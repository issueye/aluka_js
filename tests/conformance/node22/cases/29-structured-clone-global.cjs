// 全局 structuredClone 回归用例（Node 22 对拍）。
//
// 语义面刻意只用「同一性/长度/取值/错误名」判定，**回避两处已知且已登记的偏离**：
//   1) `clone instanceof Map` —— 受原型属性读面所限（Node true / Aluka false），
//      故改判 `size` 与 `get` 结果；
//   2) DataCloneError 的 message 文案（Node 含值源码文本，Aluka 为通用文案），
//      故只判 `name`。
const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

show('num', () => structuredClone(3));
show('str', () => structuredClone('s'));
show('undef', () => String(structuredClone(undefined)));
show('obj', () => JSON.stringify(structuredClone({ a: 1 })));
show('nested', () => JSON.stringify(structuredClone({ a: { b: [1, 2] } })));
show('indep', () => {
  const src = { n: 1 };
  const c = structuredClone(src);
  c.n = 2;
  return src.n + ',' + c.n;
});
show('cycle', () => {
  const o = {};
  o.self = o;
  const c = structuredClone(o);
  return c.self === c;
});
show('map', () => {
  const c = structuredClone(new Map([[1, 'a'], [2, 'b']]));
  return c.size + '|' + c.get(1) + '|' + c.get(2);
});
show('map-numstr-keys', () => {
  const c = structuredClone(new Map([[1, 'n'], ['1', 's']]));
  return c.size + '|' + c.get(1) + '|' + c.get('1');
});
show('set', () => {
  const c = structuredClone(new Set([1, '1']));
  return c.size + '|' + c.has(1) + '|' + c.has('1');
});
show('date', () => typeof structuredClone(new Date(0)));
show('regexp', () => {
  const c = structuredClone(/ab+c/gi);
  return c.source + '/' + c.flags;
});
show('transfer', () => {
  const ab = new ArrayBuffer(4);
  new Uint8Array(ab)[0] = 7;
  const c = structuredClone(ab, { transfer: [ab] });
  return 'src=' + ab.byteLength + ' cloned=' + c.byteLength + ' b0=' + new Uint8Array(c)[0];
});
show('fn-err', () => structuredClone(() => 1));
show('sym-err', () => structuredClone(Symbol('s')));
show('noargs', () => structuredClone());
