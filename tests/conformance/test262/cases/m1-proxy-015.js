
var requested = false;
var target = {};
var p = new Proxy(target, {
  setPrototypeOf: function (t, proto) { requested = true; return Reflect.setPrototypeOf(t, proto); }
});
Reflect.setPrototypeOf(p, { z: 1 });
assert.isTrue(requested, "setPrototypeOf trap invoked");
assert.sameValue(p.z, 1, "prototype applied to target");
