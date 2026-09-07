
var f = new Function("return Math.floor(4.7);");
assert.sameValue(f(), 4, "builtin global in body");
