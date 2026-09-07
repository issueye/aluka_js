
var a = [1, 2, 3, 4];
a.fill(9, 1, 3);
assert.sameValue(a.join(","), "1,9,9,4", "fill range");
var b = [1, 2, 3, 4];
b.copyWithin(0, 2);
assert.sameValue(b.join(","), "3,4,3,4", "copyWithin");
