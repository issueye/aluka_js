function calc(re, s) {
  return re.test(s) ? 1 : 0;
}
const re = /ab+c/i;
let t = 0;
for (let i = 0; i < 150; i++) {
  t += calc(re, "aBBBc".toLowerCase());
  t += calc(re, "xyz");
  const o = {};
  o["k" + (i % 3)] = i;
  t += o.k0 === undefined ? 0 : 1;
}
console.log(t);
