
assert.sameValue([2, 1].findLast(function (x) { return x > 0; }), 1, "findLast");
assert.sameValue([1, 2, 3].findIndex(function (x) { return x === 2; }), 1, "findIndex");
assert.sameValue([1, 2, 3].findLastIndex(function (x) { return x > 1; }), 2, "findLastIndex");
