
var doubled = new Int32Array([1, 2, 3]).map(function (x) { return x * 2; });
assert.sameValue(doubled.join(","), "2,4,6", "typed map");
var evens = new Int32Array([1, 2, 3, 4]).filter(function (x) { return x % 2 === 0; });
assert.sameValue(evens.join(","), "2,4", "typed filter");
