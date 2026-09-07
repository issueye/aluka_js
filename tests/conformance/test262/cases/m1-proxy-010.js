
var t = { b: 1, a: 2 };
var p = new Proxy(t, { ownKeys: function () { return ["a", "b"]; } });
assert.sameValue(Object.keys(p).join(","), "a,b", "ownKeys trap drives Object.keys");
