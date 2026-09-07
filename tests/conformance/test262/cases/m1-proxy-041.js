
var t = {};
assert.isTrue(Reflect.defineProperty(t, "x", { value: 8, writable: true, enumerable: true, configurable: true }));
assert.sameValue(t.x, 8);
