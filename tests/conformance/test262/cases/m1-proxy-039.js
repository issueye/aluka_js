
var t = {};
assert.isTrue(Reflect.setPrototypeOf(t, { tag: 1 }), "setPrototypeOf ok");
assert.sameValue(t.tag, 1, "proto applied");
