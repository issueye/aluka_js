
var a = [1, 2, 3, 4];
var removed = a.splice(1, 2);
assert.sameValue(removed.join(","), "2,3", "splice removed");
assert.sameValue(a.join(","), "1,4", "splice remainder");
