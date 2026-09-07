
var add = new Function("a", "b", "return a + b");
assert.sameValue(add(3, 4), 7, "new Function params");
