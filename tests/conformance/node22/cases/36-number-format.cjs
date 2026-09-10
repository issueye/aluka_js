// 数字格式化回归用例：toFixed / toPrecision / toExponential / toString(radix)
// 与 Number→String 的规范规则（Node 22 对拍）。
//
// 改动前 `toString(radix)` 用精确十进制展开，导致大数值的低位与 Node 不同；
// V8 的 `DoubleToRadixCString`（src/numbers/conversions.cc）全程用 f64 算术，
// 超出有效精度的高位**一律补 '0'**，本实现已逐句移植：
//   `(1e21).toString(36)` → "5v1j4f4ds7c000"（末 3 位补零，非精确展开）
//   `(1e21).toString(3)`  → 末 11 位为 '0'
//   `(1.7976931348623157e308).toString(36)` → 只有前 12 位有效
//
// 另两条**相反**的并列规则也在本用例锁定：
//   Number::toString / toExponential() ：并列取**末位为偶**者
//     `(1501199875790165.25)` 的精确展开 `…65.25` 落在 `…652`/`…653` 正中 → `…652`
//   toFixed / toPrecision             ：并列取**较大**者
//     `(1501199875790165.25).toPrecision(17)` → "1501199875790165.3"
//
// **刻意不含**的已登记偏离：locale/时区相关（本实现时区固定 UTC）、
// `Intl.NumberFormat` 表面、`console.log(-0)` 的打印形态（Node 打印 `-0`，
// 本实现打印 `0`；`String(-0)` 两侧都是 `"0"`，故此处一律用 `1 / -0` 探测符号）。
'use strict';

const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

// ---- toFixed ----
show('fixed-tie-half', () => (0.5).toFixed(0));
show('fixed-tie-2.5', () => (2.5).toFixed(0));
show('fixed-tie-3.5', () => (3.5).toFixed(0));
show('fixed-1.005', () => (1.005).toFixed(2));
show('fixed-neg-1.005', () => (-1.005).toFixed(2));
show('fixed-1.45', () => (1.45).toFixed(1));
show('fixed-1e21', () => (1e21).toFixed(0));
show('fixed-1e20', () => (1e20).toFixed(0));
show('fixed-tiny', () => (1e-7).toFixed(2));
show('fixed-denorm', () => (5e-324).toFixed(0));
show('fixed-100', () => (1).toFixed(100).length);
show('fixed-nan', () => String(NaN.toFixed(2)));
show('fixed-inf', () => String(Infinity.toFixed(2)));
show('fixed-range-hi', () => (1).toFixed(101));
show('fixed-range-neg', () => (1).toFixed(-1));

// ---- toPrecision（并列取较大）----
show('prec-tie-even', () => (1501199875790165.25).toPrecision(17));
show('prec-123.456-3', () => (123.456).toPrecision(3));
show('prec-1234.5-2', () => (1234.5).toPrecision(2));
show('prec-9.99-2', () => (9.99).toPrecision(2));
show('prec-999.9-3', () => (999.9).toPrecision(3));
show('prec-1e21-21', () => (1e21).toPrecision(21));
show('prec-1e-7-2', () => (1e-7).toPrecision(2));
show('prec-min', () => (255).toPrecision(1));
show('prec-range-hi', () => (1).toPrecision(101));
show('prec-range-lo', () => (1).toPrecision(0));

// ---- toExponential（无参并列取偶）----
show('exp-tie-even', () => (1501199875790165.25).toExponential());
show('exp-tie-even2', () => (643371375338642.25).toExponential());
show('exp-shortest-100', () => (100).toExponential());
show('exp-shortest-0.0001', () => (0.0001).toExponential());
show('exp-digits-0', () => (12345).toExponential(0));
show('exp-digits-2', () => (1234.5).toExponential(2));
show('exp-1e21', () => (1e21).toExponential());
show('exp-neg-zero', () => (-0).toExponential());
show('exp-range-hi', () => (1).toExponential(101));

// ---- base-10 Number→String（并列取偶）----
show('nstr-tie', () => String(1501199875790165.25));
show('nstr-tie2', () => String(643371375338642.25));
show('nstr-1e21', () => String(1e21));
show('nstr-1e20', () => String(1e20));
show('nstr-1e-6', () => String(1e-6));
show('nstr-1e-7', () => String(1e-7));
show('nstr-0.1+0.2', () => String(0.1 + 0.2));
show('nstr-radix10', () => (1501199875790165.25).toString(10));
show('nstr-neg-zero', () => String(-0));

// ---- toString(radix)：普通值 ----
show('r16-255', () => (255).toString(16));
show('r16-frac', () => (123.456).toString(16));
show('r16-neg', () => (-42.7).toString(16));
show('r2-255', () => (255).toString(2));
show('r2-0.1', () => (0.1).toString(2));
show('r2-0.5', () => (-0.5).toString(2));
show('r3-10', () => (10).toString(3));
show('r36-0.1', () => (0.1).toString(36));
show('r16-1.0000000000000002', () => (1.0000000000000002).toString(2));
show('r2-zero', () => (0).toString(2));
show('r36-max-safe', () => (9007199254740991).toString(36));
show('r-default-arg', () => (255).toString());
show('r-undefined-arg', () => (255).toString(undefined));
show('r-range-lo', () => (255).toString(1));
show('r-range-hi', () => (255).toString(37));
show('r-nan', () => String(NaN.toString(2)));

// ---- toString(radix)：大数值（V8 整数位丢精度补零）----
show('r36-1e21', () => (1e21).toString(36));
show('r3-1e21', () => (1e21).toString(3));
show('r16-1e21', () => (1e21).toString(16));
show('r3-1e20', () => (1e20).toString(3));
show('r36-1e100', () => (1e100).toString(36));
show('r3-1e100', () => (1e100).toString(3));
show('r36-max-double', () => (1.7976931348623157e308).toString(36));
show('r2-max-double-len', () => (1.7976931348623157e308).toString(2).length);
show('r36-neg-1e100', () => (-1e100).toString(36));

// ---- toString(radix)：极小数 / 次正规 ----
show('r3-1e-6', () => (1e-6).toString(3));
show('r36-1e-320', () => (1e-320).toString(36));
show('r2-5e-324-len', () => (5e-324).toString(2).length);
show('r2-5e-324-tail', () => (5e-324).toString(2).slice(-3));

// ---- 与既有数字表面的边界（不得回归）----
show('num-tostring', () => String(1234.5));
show('num-parseint', () => parseInt('ff', 16));
show('num-parsefloat', () => parseFloat('3.14e2'));
show('math-round-half-down', () => Math.round(0.49999999999999994));
show('math-round-neg-half', () => 1 / Math.round(-0.5));
show('math-round-2p52', () => Math.round(4503599627370497));
show('math-round-tie-neg', () => Math.round(-2.5));
show('math-round-neg-zero', () => 1 / -0);
show('num-isfinite', () => Number.isFinite(NaN));
