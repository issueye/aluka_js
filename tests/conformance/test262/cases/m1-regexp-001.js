
var m = /(?<=\$)\d+/.exec("price $100");
assert.sameValue(m[0], "100", "positive lookbehind");
assert.sameValue(m.index, 7, "match position after $");
