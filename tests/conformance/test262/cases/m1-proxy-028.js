
var validate = {
  set: function (t, k, v) {
    if (k === "age" && typeof v !== "number") { throw new TypeError("age must be number"); }
    t[k] = v;
    return true;
  }
};
var person = new Proxy({}, validate);
person.age = 30;
assert.sameValue(person.age, 30);
assert.throws(TypeError, function () { person.age = "old"; });
