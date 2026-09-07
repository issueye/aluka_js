
var sp = [1, 2, 3].toSpliced(1, 1, 9);
assert.sameValue(sp.join(","), "1,9,3", "toSpliced");
