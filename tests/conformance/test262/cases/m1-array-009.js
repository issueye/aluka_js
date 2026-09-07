
var arr = [10, 20];
assert.sameValue(arr.toString(), "10,20", "array toString");
var it = arr.values();
var first = it.next();
assert.sameValue(first.value, 10, "values iterator");
assert.isFalse(first.done, "values not done");
