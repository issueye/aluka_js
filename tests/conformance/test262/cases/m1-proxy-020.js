
var t = { a: 1 };
var p = new Proxy(t, {});
p.a = 9;
assert.sameValue(t.a, 9, "no-trap set forwards");
assert.sameValue(Object.keys(p).join(","), "a", "no-trap keys");
