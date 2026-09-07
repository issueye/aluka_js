
var f = new Function("return 5");
assert.sameValue(f(), 5, "new Function body");
