
var t = { x: 1, y: 2 };
var p = new Proxy(t, { ownKeys: function (target) { return Object.keys(target).reverse(); } });
var acc = "";
for (var k in p) { acc += k; }
assert.sameValue(acc, "yx", "ownKeys trap drives for-in order");
