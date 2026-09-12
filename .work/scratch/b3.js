function hash(x) {
  let h = x | 0;
  h = (h << 3) ^ (h >>> 2);
  h = h & 0xff;
  h = h | 0x100;
  h = h ^ 0x55;
  return ~h;
}
let acc = 0;
for (let i = 0; i < 150; i++) {
  acc += hash(i * 7);
  acc |= 1;
  acc &= 0xffff;
  acc ^= i;
  acc >>= 1;
  acc >>>= 0;
  acc <<= 0;
  acc += ~i;
  if (i < 3 || i > 146) { console.log(i, acc); }
}
