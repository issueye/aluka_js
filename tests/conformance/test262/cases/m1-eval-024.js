
var r = eval("var sum = 0; sum += 1; sum += 2; sum");
assert.sameValue(r, 3, "compound assignment sequence");
