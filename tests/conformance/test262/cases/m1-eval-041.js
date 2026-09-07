
var f = new Function("a", "b", "c", "return a + b + c;");
assert.sameValue(f(1, 2, 3), 6, "three params");
assert.sameValue(f.length, 3, "Function.length reflects params");
