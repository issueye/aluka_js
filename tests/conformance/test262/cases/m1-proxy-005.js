
var seen = [];
var t = {};
var p = new Proxy(t, { get: function (target, key, receiver) { seen.push(key); return receiver === p ? "self" : "other"; } });
assert.sameValue(p.q, "self", "receiver is the proxy");
assert.sameValue(seen.join(","), "q");
