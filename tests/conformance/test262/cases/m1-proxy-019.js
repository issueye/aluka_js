
var defined = [];
var t = {};
var p = new Proxy(t, { defineProperty: function (target, key, desc) { defined.push(key); return Reflect.defineProperty(target, key, desc); } });
Reflect.defineProperty(p, "q", { value: 3, writable: true, enumerable: true, configurable: true });
assert.sameValue(t.q, 3, "defineProperty forwarded");
assert.sameValue(defined.join(","), "q");
