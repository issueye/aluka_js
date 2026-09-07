
var g = Object.groupBy([1, 2, 3, 4], function (x) { return x % 2 === 0 ? "even" : "odd"; });
assert.sameValue(g.odd.join(","), "1,3", "Object.groupBy odd");
assert.sameValue(g.even.join(","), "2,4", "Object.groupBy even");
