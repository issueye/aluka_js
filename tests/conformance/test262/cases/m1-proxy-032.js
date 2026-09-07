
var t = { secret: 99 };
var p = new Proxy(t, { get: function (target, key) { return key === "secret" ? "***" : target[key]; } });
assert.sameValue(Reflect.get(p, "secret"), "***", "Reflect.get respects get trap");
