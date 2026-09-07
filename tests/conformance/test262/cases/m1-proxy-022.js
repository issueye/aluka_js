
var pair = Proxy.revocable({}, {});
pair.revoke();
assert.throws(TypeError, function () { pair.proxy.anything = 1; });
