
var t = { gone: 1, keep: 2 };
var p = new Proxy(t, { deleteProperty: function (target, key) { return delete target[key]; } });
delete p.gone;
assert.sameValue(t.gone, undefined, "deleteProperty forwarded");
assert.sameValue(t.keep, 2, "other key untouched");
