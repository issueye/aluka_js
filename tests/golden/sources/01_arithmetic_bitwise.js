const a = 10 + 20 - 5 * 2 / 1;
const b = (a % 7) ** 2;
const c = (b & 15) | (a ^ 3);
const d = (c << 2) >> 1;
const e = d >>> 1;
const f = ~e;
const g = -f;
const h = +g;
console.log(a, b, c, d, e, f, g, h);
