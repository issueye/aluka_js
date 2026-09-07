
var proto = { marker: true };
var p = new Proxy({}, { getPrototypeOf: function () { return proto; } });
assert.sameValue(Reflect.getPrototypeOf(p), proto, "getPrototypeOf trap drives Reflect.getPrototypeOf");
var p2 = new Proxy({}, { getPrototypeOf: function () { return Array.prototype; } });
assert.isTrue(p2 instanceof Array, "trap-driven instanceof");
