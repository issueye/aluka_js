function makeAcc(start) {
  let acc = start;
  return function add(n) { acc += n; return acc; };
}
const add = makeAcc(100);
let t = 0;
for (let i = 0; i < 200; i++) { t = add(i); }
console.log(t, 100 + (199 * 200) / 2);
