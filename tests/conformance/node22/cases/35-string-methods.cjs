// 字符串方法回归用例：padStart / padEnd / at / codePointAt（Node 22 对拍）。
//
// 改前四者在 `String.prototype` 面上**已注册但无实现**（`call_string_method`
// 未覆盖）→ 调用得 `undefined`：
//   `"abc".padStart(5, "*")` → undefined（Node "**abc"）
//   `"abc".padEnd(5, "-")`   → undefined（Node "abc--"）
//   `"abc".at(-1)`           → undefined（Node "c"）
//   `"a".codePointAt(0)`     → undefined（Node 97）
// `cases/gen/deviations/` 中 padStart/padEnd 共 10 例、at/codePointAt 各 1–2 例分歧。
//
// **刻意不含**的已登记偏离（本实现字符串按**码点**寻址，Node 按 **UTF-16 码元**）：
//   `"😀".length`（应 2）、`"😀".codePointAt(1)`（Node 返回低位代理 0xDE00）、
//   `"😀".at(1)`、padStart 的 targetLength 对代理对的计数。
// 这些需系统性改造（Rust String 无法表示孤立代理），另行登记。
'use strict';

const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

// ---- padStart ----
show('padStart-basic', () => 'abc'.padStart(5, '*'));
show('padStart-nofill', () => '[' + 'abc'.padStart(5) + ']');
show('padStart-numeric', () => '5'.padStart(2, '0'));
show('padStart-already-long', () => 'abcdef'.padStart(3, '*'));
show('padStart-equal', () => 'abc'.padStart(3, '*'));
show('padStart-fill-longer', () => 'a'.padStart(4, 'xy'));
show('padStart-fill-cycles', () => 'a'.padStart(7, 'xyz'));
show('padStart-empty-recv', () => ''.padStart(3, 'z'));
show('padStart-zero', () => 'abc'.padStart(0, '*'));
show('padStart-empty-fill', () => 'abc'.padStart(5, ''));
show('padStart-numeric-fill', () => 'a'.padStart(3, 0));
show('padStart-unicode-fill', () => 'a'.padStart(3, '\u00e9'));

// ---- padEnd ----
show('padEnd-basic', () => 'abc'.padEnd(5, '-'));
show('padEnd-nofill', () => '[' + 'abc'.padEnd(5) + ']');
show('padEnd-already-long', () => 'abcdef'.padEnd(3, '-'));
show('padEnd-fill-longer', () => 'a'.padEnd(4, 'xy'));
show('padEnd-empty-recv', () => ''.padEnd(3, 'z'));
show('padEnd-zero', () => 'abc'.padEnd(0, '-'));
show('padEnd-empty-fill', () => 'abc'.padEnd(5, ''));

// ---- at ----
show('at-negative', () => 'abc'.at(-1));
show('at-negative-mid', () => 'abc'.at(-2));
show('at-positive', () => 'abc'.at(1));
show('at-zero', () => 'abc'.at(0));
show('at-oob', () => String('abc'.at(9)));
show('at-oob-negative', () => String('abc'.at(-9)));
show('at-empty', () => String(''.at(0)));
show('at-nan', () => String('abc'.at(NaN)));
show('at-float', () => 'abc'.at(1.7));

// ---- codePointAt ----
show('cpa-bmp', () => 'a'.codePointAt(0));
show('cpa-numeric', () => '5'.codePointAt(0));
show('cpa-mid', () => 'abc'.codePointAt(2));
show('cpa-oob', () => String('a'.codePointAt(5)));
show('cpa-negative', () => String('a'.codePointAt(-1)));
show('cpa-nan', () => String('a'.codePointAt(NaN)));
show('cpa-empty', () => String(''.codePointAt(0)));

// ---- .call 形态（复用同一分派）----
show('padStart-call', () => String.prototype.padStart.call('7', 3, '0'));
show('at-call', () => String.prototype.at.call('xyz', -1));
show('cpa-call', () => String.prototype.codePointAt.call('A', 0));
show('padEnd-call', () => String.prototype.padEnd.call('7', 3, '.'));

// ---- 与既有字符串方法的边界（不得回归）----
show('includes', () => 'abc'.includes('b'));
show('repeat', () => 'ab'.repeat(3));
show('trimStart', () => '[' + '  a'.trimStart() + ']');
show('slice', () => 'abcdef'.slice(1, 3));
show('charAt', () => 'abc'.charAt(1));
show('charCodeAt', () => 'a'.charCodeAt(0));
show('indexOf', () => 'abc'.indexOf('c'));
show('toUpperCase', () => 'ab'.toUpperCase());
