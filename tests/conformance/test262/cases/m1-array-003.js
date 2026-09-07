
assert.isTrue([1, 2, 3].every(function (x) { return x > 0; }), "every true");
assert.isFalse([1, -2].every(function (x) { return x > 0; }), "every false");
assert.isTrue([1, -2].some(function (x) { return x < 0; }), "some true");
