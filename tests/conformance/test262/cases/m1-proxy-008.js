
var hits = [];
var t = { a: 1 };
var p = new Proxy(t, { has: function (target, key) { hits.push(key); return key !== "a"; } });
assert.isTrue("b" in p, "has trap true path");
assert.isFalse("a" in p, "has trap false path");
assert.sameValue(hits.join(","), "b,a");
