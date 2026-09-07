
var i32 = new Int32Array([1, 2, 3]);
var cp = i32.slice(0, 2);
cp[0] = 77;
assert.sameValue(i32[0], 1, "slice copies");
assert.sameValue(cp.join(","), "77,2", "copied values");
