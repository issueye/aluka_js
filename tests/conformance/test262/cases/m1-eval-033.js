
function outer() {
  var secret = 5;
  function inner() { return eval("secret * 2"); }
  return inner();
}
assert.sameValue(outer(), 10, "upvalue penetration in nested fn");
