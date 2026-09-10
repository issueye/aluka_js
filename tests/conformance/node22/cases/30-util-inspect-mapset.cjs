// util.inspect 的 Map/Set 格式化回归用例（Node 22 对拍）。
//
// 改前 `inspect_value` 无 Map/Set 分支 → 落到 `[object Object]`；现按 Node 形态输出
// `Map(n) { k => v }` / `Set(n) { v }`，空集合为 `Map(0) {}` / `Set(0) {}`，
// 条目级字符串加单引号。
const util = require('node:util');
const show = (label, v) => console.log(label + ':', util.inspect(v));

show('map1', new Map([[1, 'a']]));
show('map-empty', new Map());
show('set2', new Set([1, 2]));
show('set-empty', new Set());
show('map-nested', new Map([['k', { x: 1 }]]));
show('set-str', new Set(['s']));
show('map2', new Map([[1, 'a'], [2, 'b']]));
show('map-numstr-keys', new Map([[1, 'n'], ['1', 's']]));
show('set-numstr', new Set([1, '1']));
show('plain-obj', { a: 1 });
show('plain-arr', [1, 2]);
