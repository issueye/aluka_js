function classify(v) {
  const t = typeof v;
  return t === "number" ? +v + 1 : 0;
}
function sumElems(arr) {
  let s = 0;
  for (let i = 0; i < arr.length; i++) { if (arr[i] !== undefined) { s += arr[i]; } }
  return s;
}
function buildArr(n) {
  const a = [];
  for (let i = 0; i < n; i++) { a.push(i + 1); }
  return a;
}
function protoCheck(o) {
  return Object.getPrototypeOf(o) !== undefined;
}
let total = 0;
const arr = [];
for (let i = 0; i < 120; i++) {
  total += classify(i / 8);
  arr.push(i);
  total += sumElems(arr);
  total += protoCheck(arr) ? 1 : 0;
  total += (1 in arr) ? 1 : 0;
  total += (arr instanceof Object) ? 1 : 0;
  if (i % 3 === 0) { delete arr[i]; total += arr[i] === undefined ? 2 : 0; }
  total += (typeof total === "number") ? +1 : 0;
}
const o = { a: 1, b: 2 };
o.a = 10;
total += o.a + o.b;
delete o.b;
total += o.b === undefined ? 5 : 0;
total += protoCheck(o) ? 3 : 0;
const mixed = buildArr(50);
for (let i = 0; i < 120; i++) { mixed.push(i); total += mixed[50]; }
console.log(total, arr.length, mixed.length, o.b);
