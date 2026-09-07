
var t = { a: 1 };
var p = new Proxy(t, {});
assert.sameValue(p.a, 1, "get passthrough");
p.b = 2;
assert.sameValue(t.b, 2, "set passthrough");
assert.isTrue("a" in p, "has passthrough");
