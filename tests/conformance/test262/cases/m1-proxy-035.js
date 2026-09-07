
var t = { doomed: 1 };
assert.isTrue(Reflect.deleteProperty(t, "doomed"));
assert.sameValue(t.doomed, undefined);
