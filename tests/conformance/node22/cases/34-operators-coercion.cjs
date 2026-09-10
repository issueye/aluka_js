// 运算符强制转换与关系比较回归用例（Node 22 对拍）。
//
// 改前三组**核心语义静默错值**（均由 `cases/gen/deviations/` 暴露）：
//   1) 关系比较 `Op::Lt/Le/Gt/Ge` 直接做纯数值比较 → 字符串比较恒 false：
//        `"a" < "b"` → false（Node true）、`"A" < "a"` → false、
//        `"2" > "10"` → false、`1 < "2"` → false（字符串未被 ToNumber）
//   2) `add_values` 缺「双方皆原始值且非字符串 → 数值相加」这一步 → 落到 NaN：
//        `true + 1` → NaN（Node 2）、`true + true` → NaN（Node 2）、
//        `null + 1` → NaN（Node 1）
//   3) 位运算 `BitNot/BitAnd/BitOr/BitXor` 用字符串不感知的 `to_number`：
//        `"5" | 0` → 0（Node 5）、`"3" & 1` → 0（Node 1）、`~"5"` → -1（Node -6）
//
// 本用例只覆盖**已修复**的语义；以下仍为已知缺口，刻意不含：
//   `"😀".length`（本实现按码点计数，Node 按 UTF-16 码元 = 2）、
//   `toFixed` / `toPrecision` / `toExponential` / `toString(radix)` 的小数形态。
'use strict';

const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

// ---- 关系比较：字符串按 UTF-16 码元序 ----
show('cmp-str-lt', () => 'a' < 'b');
show('cmp-str-upper', () => 'A' < 'a');
show('cmp-str-num-like', () => '2' > '10');
show('cmp-str-prefix', () => 'abc' < 'abd');
show('cmp-str-longer', () => 'ab' < 'abc');
show('cmp-str-eq-le', () => 'a' <= 'a');
show('cmp-str-ge', () => 'b' >= 'a');
show('cmp-str-false', () => 'b' < 'a');
// ---- 关系比较：混合类型走 ToNumber ----
show('cmp-mixed-lt', () => 1 < '2');
show('cmp-mixed-le', () => 2 <= '2');
show('cmp-mixed-gt', () => '3' > 2);
show('cmp-num', () => 1 < 2);
show('cmp-bool', () => true > false);
show('cmp-null', () => null < 1);
show('cmp-undef-nan', () => undefined < 1);
show('cmp-nan-both', () => NaN < 1);
show('cmp-nan-le', () => NaN <= NaN);
// ---- 关系比较：对象经 ToPrimitive（数组 join / 对象默认）----
show('cmp-arr', () => [1] < [2]);
show('cmp-arr-str', () => ['a'] < ['b']);
show('cmp-obj', () => ({}) < 1);
show('cmp-arr-mixed', () => [2] > 1);
// ---- 加法：原始值数值相加 ----
show('add-true-1', () => true + 1);
show('add-true-true', () => true + true);
show('add-false-1', () => false + 1);
show('add-null-1', () => null + 1);
show('add-null-null', () => null + null);
show('add-undef', () => String(undefined + 1));
show('add-undef-undef', () => String(undefined + undefined));
// ---- 加法：字符串拼接优先 ----
show('add-str-num', () => '1' + 2);
show('add-num-str', () => 2 + '1');
show('add-str-bool', () => 'v=' + true);
show('add-str-null', () => 'v=' + null);
show('add-str-undef', () => 'v=' + undefined);
// ---- 位运算：字符串/布尔/null 强制转换 ----
show('bit-or', () => '5' | 0);
show('bit-xor', () => '5' ^ 0);
show('bit-and', () => '3' & 1);
show('bit-not', () => ~'5');
show('bit-not-zero', () => ~0);
show('bit-or-bool', () => true | 0);
show('bit-and-null', () => null | 0);
show('bit-or-nan', () => NaN | 0);
show('bit-or-undef', () => undefined | 0);
show('shift-left', () => '2' << 3);
show('shift-right', () => '16' >> 2);
show('shift-right-unsigned', () => '-16' >>> 28);
show('bit-or-hex-str', () => 'ff' | 0);
// ---- 与既有语义的边界（不得回归）----
show('sub-str', () => '5' - '2');
show('mul-bool', () => true * 3);
show('div-str', () => '6' / '2');
show('mod-str', () => '7' % '4');
show('unary-plus-str', () => +'3');
show('unary-minus-bool', () => -true);
show('strict-eq', () => 'a' === 'a');
show('loose-eq-numstr', () => '1' == 1);
show('truthy-empty-str', () => !'');
show('ternary-coerce', () => (true + 1 > 2 ? 'yes' : 'no'));
