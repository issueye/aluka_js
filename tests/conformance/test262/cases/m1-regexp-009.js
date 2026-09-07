
assert.isTrue(/\bcat\b/.test("a cat here"), "word boundary");
assert.isFalse(/\bcat\b/.test("concat"), "no boundary inside word");
