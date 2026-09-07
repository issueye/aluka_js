
function f() { var c = 1; eval("c = 99"); return c; }
assert.sameValue(f(), 99, "direct eval writes enclosing var");
