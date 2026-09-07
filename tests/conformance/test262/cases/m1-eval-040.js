
var f = new Function("");
assert.sameValue(f(), undefined, "empty body returns undefined");
