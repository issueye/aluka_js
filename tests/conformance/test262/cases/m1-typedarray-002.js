
var i32 = new Int32Array(4);
i32[0] = 42;
i32[3] = -7;
assert.sameValue(i32[0], 42, "write read back");
assert.sameValue(i32[3], -7, "negative value");
assert.sameValue(i32.length, 4, "length");
