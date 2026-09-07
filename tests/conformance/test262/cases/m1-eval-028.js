
var picked = eval("var src = [10, 20, 30]; src.filter(function (x) { return x > 15; }).join(',')");
assert.sameValue(picked, "20,30", "filter inside eval");
