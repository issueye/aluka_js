
function fn(x) { return x * 2; }
var pf = new Proxy(fn, { apply: function (target, thisArg, args) { return Reflect.apply(target, thisArg, args) + 1; } });
assert.sameValue(pf(10), 21, "apply trap adds one");
