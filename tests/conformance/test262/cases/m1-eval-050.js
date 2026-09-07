
function f() { eval("function declared() { return 77; }"); return declared(); }
assert.sameValue(f(), 77, "function declaration inside eval callable");
