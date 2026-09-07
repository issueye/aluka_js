
var f = new Function("n", "var r = 1; for (var i = 1; i <= n; i++) { r *= i; } return r;");
assert.sameValue(f(5), 120, "loop in body computes factorial");
