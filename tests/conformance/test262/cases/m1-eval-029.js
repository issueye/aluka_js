
function f() { var x = 7; return eval("x + 1"); }
assert.sameValue(f(), 8, "reads var");
