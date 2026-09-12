function hash(x) {
  let h = x | 0;
  h = (h << 3) ^ (h >>> 2);
  h = h & 0xff;
  h = h | 0x100;
  h = h ^ 0x55;
  return ~h;
}
for (let i = 0; i < 6; i++) { console.log(hash(i * 7)); }
console.log((5 | 3), (5 & 3), (5 ^ 3), (5 << 1), (-8 >> 1), (-8 >>> 28), (~5));
