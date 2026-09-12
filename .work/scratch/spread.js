function cat(a, b) {
  return [...a, ...b].length;
}
let t = 0;
for (let i = 0; i < 150; i++) {
  t += cat([1, 2], [3]);
  t += cat([], [i]);
}
console.log(t);
