
var pair = Proxy.revocable({ x: 1 }, {});
assert.sameValue(pair.proxy.x, 1, "revocable works before revoke");
assert.sameValue(typeof pair.revoke, "function", "revoke is function");
pair.revoke();
assert.throws(TypeError, function () { pair.proxy.x; });
