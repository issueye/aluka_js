
var f = new Function("return [1, 2, 3].reduce(function (a, b) { return a + b; }, 0);");
assert.sameValue(f(), 6, "array ops in body");
