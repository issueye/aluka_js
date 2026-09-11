// 覆盖率探针：3 行执行的语句 + 1 行未执行的语句（else 分支）
function add(a, b) {
  return a + b;
}
function never(x) {
  return x * 100;
}
let total = 0;
for (let i = 0; i < 2; i++) {
  if (i % 2 === 0) {
    total += add(i, 1);
  } else {
    total += 10;
  }
}
console.log('total', total);
