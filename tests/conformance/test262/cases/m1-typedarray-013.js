
var s = new Int32Array([3, 1, 2]).sort();
assert.sameValue(s.join(","), "1,2,3", "numeric sort");
var r = new Int16Array([1, 2, 3]).reverse();
assert.sameValue(r.join(","), "3,2,1", "reverse in place");
