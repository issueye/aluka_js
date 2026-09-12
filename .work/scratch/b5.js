function hash(x) {
  let h = x | 0;
  h = (h << 3) ^ (h >>> 2);
  h = h & 0xff;
  h = h | 0x100;
  h = h ^ 0x55;
  return ~h;
}
console.log("h0", hash(0));
let t1 = -342; t1 |= 1; console.log("or", t1);
let t2 = -341; t2 &= 0xffff; console.log("and", t2);
let t3 = 65195; t3 ^= 0; console.log("xor", t3);
let t4 = 65195; t4 >>= 1; console.log("shr", t4);
let t5 = 32597; t5 >>>= 0; console.log("ushr", t5);
let t6 = 32597; t6 <<= 0; console.log("shl", t6);
let t7 = 32596; t7 += ~0; console.log("not", t7);
