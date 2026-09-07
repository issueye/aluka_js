
var adder = new Function("x", "return function (y) { return x + y; };");
var add5 = adder(5);
assert.sameValue(add5(3), 8, "currying via Function constructor");
