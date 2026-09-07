
var log = [];
var t = { a: 1 };
var p = new Proxy(t, {
  get: function (t2, k) { log.push("get:" + k); return t2[k]; },
  set: function (t2, k, v) { log.push("set:" + k); t2[k] = v; return true; }
});
p.b = 2;
var v = p.a;
assert.sameValue(log.join(","), "set:b,get:a", "trap order recorded");
assert.sameValue(v, 1);
