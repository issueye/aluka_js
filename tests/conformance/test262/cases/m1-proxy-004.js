
var log = [];
var t = { x: 42 };
var p = new Proxy(t, { get: function (target, key) { log.push(key); return target[key]; } });
assert.sameValue(p.x, 42);
assert.sameValue(log.join(","), "x", "get trap fired with key");
