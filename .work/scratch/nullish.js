// OptionalJump (?.) / JmpNullishKeep (??) 热函数覆盖
function pick(o) {
  return o?.v ?? 7;
}
function len(o) {
  return o?.list?.length ?? -1;
}
let a = 0;
let b = 0;
let c = 0;
const objs = [{ v: 1 }, {}, null, { v: 5, list: [1, 2, 3] }, undefined];
for (let i = 0; i < 300; i++) {
  const o = objs[i % 5];
  a += pick(o);
  b += len(o);
  c += o?.missing ?? 9;
}
console.log(a, b, c);
