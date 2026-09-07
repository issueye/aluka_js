
function f(a, b) { return eval("a - b"); }
assert.sameValue(f(9, 4), 5, "reads parameters");
