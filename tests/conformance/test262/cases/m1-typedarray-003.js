
var ab = new ArrayBuffer(8);
var i32 = new Int32Array(ab);
i32[0] = 1;
var u8 = new Uint8Array(ab);
assert.sameValue(u8[0], 1, "LE low byte");
assert.sameValue(u8[1], 0, "LE high bytes zero");
