
assert.sameValue([1, 2].concat([3, 4]).join(","), "1,2,3,4", "concat arrays");
assert.sameValue([1].concat(2, [3]).join(","), "1,2,3", "concat mixed");
