function sumElems(arr) {
  let s = 0;
  for (let i = 0; i < arr.length; i++) { s += arr[i]; }
  return s;
}
const a = [];
for (let i = 1; i <= 100; i++) { a.push(i); }
delete a[3];
let t = 0;
for (let i = 0; i < 120; i++) { t += sumElems(a); }
console.log(t);
