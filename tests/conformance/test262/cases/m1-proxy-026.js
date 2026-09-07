
var calls = 0;
var target = function () { calls += 1; return calls; };
var p = new Proxy(target, {});
p();
p();
assert.sameValue(calls, 2, "no-trap apply forwards");
