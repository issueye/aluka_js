
function f() { var a = 10; var b = 20; return eval("a + b"); }
assert.sameValue(f(), 30, "direct eval reads enclosing scope");
