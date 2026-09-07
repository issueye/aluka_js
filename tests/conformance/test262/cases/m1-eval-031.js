
function f() { var n = 1; eval("n = n + 41"); return n; }
assert.sameValue(f(), 42, "writes var");
