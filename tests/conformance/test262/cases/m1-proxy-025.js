
var arr = [10, 20, 30];
var p = new Proxy(arr, {});
assert.sameValue(p[1], 20, "array index through proxy");
assert.sameValue(p.length, 3, "length through proxy");
