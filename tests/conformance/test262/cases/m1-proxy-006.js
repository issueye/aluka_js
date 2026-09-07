
var writes = [];
var t = {};
var p = new Proxy(t, { set: function (target, key, value) { writes.push(key + "=" + value); target[key] = value; return true; } });
p.n = 7;
assert.sameValue(t.n, 7, "set trap forwarded");
assert.sameValue(writes.join(","), "n=7");
