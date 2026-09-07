
var m = /(?<=(a+))b/.exec("aaab");
assert.sameValue(m[1], "aaa", "greedy lookbehind capture");
