
var t = {};
assert.isTrue(Reflect.set(t, "k", 5), "Reflect.set returns true");
assert.sameValue(t.k, 5);
