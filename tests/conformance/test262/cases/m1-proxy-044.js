
var methods = ["apply", "construct", "defineProperty", "deleteProperty", "get",
  "getOwnPropertyDescriptor", "getPrototypeOf", "has", "isExtensible", "ownKeys",
  "preventExtensions", "set", "setPrototypeOf"];
var all = true;
for (var i = 0; i < methods.length; i++) {
  if (typeof Reflect[methods[i]] !== "function") { all = false; }
}
assert.isTrue(all, "all 13 Reflect methods are functions");
assert.sameValue(methods.length, 13, "13 methods listed");
