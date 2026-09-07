
var mk = eval("(function (a) { return function (b) { return a + b; }; })");
assert.sameValue(mk(1)(2), 3, "eval returns closure");
