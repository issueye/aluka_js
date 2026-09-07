
var d = Reflect.getOwnPropertyDescriptor({ v: 3 }, "v");
assert.sameValue(d.value, 3, "descriptor value");
assert.isTrue(d.writable && d.enumerable && d.configurable, "descriptor flags");
assert.sameValue(Reflect.getOwnPropertyDescriptor({}, "nope"), undefined, "missing descriptor");
