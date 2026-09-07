
var t = { deep: { v: 1 } };
var p = new Proxy(t, {});
assert.sameValue(p.deep.v, 1, "nested get through proxy");
