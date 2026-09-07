
var o = { x: 1, y: 2 };
var keys = Reflect.ownKeys(o);
assert.sameValue(keys.sort().join(","), "x,y", "Reflect.ownKeys keys");
