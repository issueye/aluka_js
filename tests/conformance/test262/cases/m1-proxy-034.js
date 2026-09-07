
var t = { a: 1 };
assert.isTrue(Reflect.has(t, "a"), "Reflect.has present");
assert.isFalse(Reflect.has(t, "b"), "Reflect.has absent");
