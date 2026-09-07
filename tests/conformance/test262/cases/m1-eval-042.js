
var f = Function("return 8;");
assert.sameValue(f(), 8, "callable without new");
