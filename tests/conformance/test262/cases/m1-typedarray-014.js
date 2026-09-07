
var sum = 0;
var ta = new Int32Array([1, 2, 3]);
for (var v of ta) { sum += v; }
assert.sameValue(sum, 6, "for-of over typed array");
var keys = new Int32Array([9]).keys();
var step = keys.next();
assert.sameValue(step.value, 0, "keys iterator index");
assert.sameValue(step.done, false, "iterator not done");
