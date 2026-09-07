
var a = Int32Array.from([5, 6, 7]);
assert.sameValue(a.join(","), "5,6,7", "from");
var b = Uint8Array.of(9, 8);
assert.sameValue(b.join(","), "9,8", "of");
