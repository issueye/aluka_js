
function make() { var base = 100; return function (d) { return eval("base - d"); }; }
assert.sameValue(make()(15), 85, "closure factory with eval");
