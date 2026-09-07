
var w = [1, 2, 3].with(1, 9);
assert.sameValue(w.join(","), "1,9,3", "with replaces");
