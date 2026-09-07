
var p = new Proxy({ v: 5 }, { getOwnPropertyDescriptor: function (t, key) {
  var d = Reflect.getOwnPropertyDescriptor(t, key);
  if (d) { d.writable = false; }
  return d;
}});
var d = Reflect.getOwnPropertyDescriptor(p, "v");
assert.sameValue(d.writable, false, "trap-mutated descriptor");
assert.sameValue(d.value, 5, "descriptor value passthrough");
