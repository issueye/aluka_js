
var cnt = eval("var c = 0; for (var i = 0; i < 4; i++) { c += 1; } c");
assert.sameValue(cnt, 4, "for loop with counter var");
