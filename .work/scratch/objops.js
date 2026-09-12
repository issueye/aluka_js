function hotSpread(src) {
  const o = { ...src, extra: 1 };
  let s = 0;
  for (const k in o) { s += 1; }
  return s + o.extra;
}
const src = { a: 1, b: 2, c: 3 };
let t = 0;
for (let i = 0; i < 150; i++) { t += hotSpread(src); }
const obj = { get x() { return 42; } };
console.log(t, obj.x);
