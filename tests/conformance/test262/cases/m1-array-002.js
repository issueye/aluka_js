
assert.sameValue([1, 2, 3].indexOf(2), 1, "indexOf");
assert.sameValue([1, 2, 3].indexOf(9), -1, "indexOf missing");
assert.isTrue([1, 2].includes(2), "includes");
assert.isFalse([1, 2].includes(3), "includes missing");
