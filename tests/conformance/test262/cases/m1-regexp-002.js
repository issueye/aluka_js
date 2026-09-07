
var m = /(?<!\$)\b\d+/.exec("100 vs $20");
assert.sameValue(m[0], "100", "negative lookbehind skips $20");
