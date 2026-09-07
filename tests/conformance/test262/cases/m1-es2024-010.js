
var m = Map.groupBy(["a", "bb", "cc"], function (x) { return x.length; });
assert.sameValue(m.get(1).join(","), "a", "Map.groupBy single");
assert.sameValue(m.get(2).join(","), "bb,cc", "Map.groupBy pair");
assert.isTrue("abc".isWellFormed(), "isWellFormed true");
assert.sameValue("abc".toWellFormed(), "abc", "toWellFormed passthrough");
