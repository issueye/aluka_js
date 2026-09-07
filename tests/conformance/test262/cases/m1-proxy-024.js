
var t = { m: 1, n: 2 };
var p = new Proxy(t, {});
var copy = {};
for (var k in p) { copy[k] = t[k]; }
assert.sameValue(copy.m, 1, "for-in copy via proxy");
assert.sameValue(copy.n, 2);
