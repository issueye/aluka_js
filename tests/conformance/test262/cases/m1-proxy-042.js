
function sum(a, b) { return a + b; }
assert.sameValue(Reflect.apply(sum, null, [2, 3]), 5, "Reflect.apply");
