
function callable() { return 1; }
var p = new Proxy(callable, {});
assert.sameValue(typeof p, "function", "proxy of function is function");
var po = new Proxy({}, {});
assert.sameValue(typeof po, "object", "proxy of object is object");
