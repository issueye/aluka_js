
var base = { m: 1 };
var child = Object.create(base);
assert.sameValue(Reflect.getPrototypeOf(child), base, "getPrototypeOf chain");
