
assert.sameValue([1, [2, 3]].flat().join(","), "1,2,3", "flat");
assert.sameValue([1, 2].flatMap(function (x) { return [x * 10]; }).join(","), "10,20", "flatMap");
