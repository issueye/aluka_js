
var t = { a: 1 };
var log = [];
var p = new Proxy(t, {
  get: function (t2, k) { log.push("get"); return Reflect.get(t2, k); },
  has: function (t2, k) { log.push("has"); return Reflect.has(t2, k); }
});
var g = p.a;
var h = "a" in p;
assert.sameValue(log.join(","), "get,has", "Reflect.* inside traps");
assert.sameValue(g, 1);
assert.isTrue(h);
