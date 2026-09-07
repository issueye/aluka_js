
var i32 = new Int32Array([1, 2, 3, 4]);
var sub = i32.subarray(1, 3);
assert.sameValue(sub.length, 2, "subarray length");
sub[0] = 99;
assert.sameValue(i32[1], 99, "subarray shares buffer");
