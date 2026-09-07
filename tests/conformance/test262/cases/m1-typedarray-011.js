
var i32 = new Int32Array(3);
i32.set([7, 8]);
assert.sameValue(i32[0], 7, "set from array");
assert.sameValue(i32[1], 8);
i32.set([9], 2);
assert.sameValue(i32[2], 9, "set with offset");
