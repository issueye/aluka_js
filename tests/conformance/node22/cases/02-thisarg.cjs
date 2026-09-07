// N22-A2：Array.prototype 方法 thisArg 对非箭头函数生效。
const o = { m: 2 };
const r = [
  [1, 2].map(function (x) { return x * this.m; }, o).join(','),
  [1, 2, 3].filter(function (x) { return x > this.m; }, o).join(','),
  [1, 2, 3].find(function (x) { return x > this.m; }, o),
  // reduce/reduceRight 无 thisArg 参数（第三参数被忽略，Node 语义）。
  [1, 2, 3].reduce(function (a, x) { return a + x; }, 0, o),
  [1, 2, 3].reduceRight(function (a, x) { return a + x; }, 0, o),
  [1, 2, 3].some(function (x) { return x > this.m; }, o),
  [1, 2].forEach ? 'fe' : 'no-fe',
].join('|');
console.log('result: ' + r);
