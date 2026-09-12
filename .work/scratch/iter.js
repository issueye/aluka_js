function sumIt(it) {
  let s = 0;
  for (const x of it) { s += x; }
  return s;
}
function sumStr(s) {
  let n = 0;
  for (const ch of s) { n += ch === "a" ? 1 : 0; }
  return n;
}
let total = 0;
for (let i = 0; i < 120; i++) {
  total += sumIt([1, 2, i]);
  total += sumIt(new Set([5, i]));
  total += sumStr("banana" + i);
}
console.log(total);
