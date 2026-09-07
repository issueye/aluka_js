
var asked = false;
var p = new Proxy({}, { preventExtensions: function (t) { asked = true; return Reflect.preventExtensions(t); } });
assert.isTrue(Reflect.preventExtensions(p), "preventExtensions trap returns true");
assert.isTrue(asked, "trap invoked");
