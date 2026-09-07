
function outer() {
  var counter = 0;
  var step = function () { eval("counter += 2"); };
  step();
  step();
  return counter;
}
assert.sameValue(outer(), 4, "eval writes via shared upvalue");
