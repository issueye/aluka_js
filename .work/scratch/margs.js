function run(obj, args) {
  return obj.sum(...args);
}
const o = {
  sum: function (...xs) { return xs.reduce((a, b) => a + b, 0); },
};
let t = 0;
for (let i = 0; i < 150; i++) {
  t += run(o, [1, 2, i]);
}
console.log(t);
