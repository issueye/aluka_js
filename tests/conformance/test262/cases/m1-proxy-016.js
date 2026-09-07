
var p = new Proxy({}, { isExtensible: function () { return true; } });
assert.isTrue(Reflect.isExtensible(p), "isExtensible trap");
