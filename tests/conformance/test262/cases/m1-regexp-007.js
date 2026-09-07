
assert.isTrue(/(?<w>\w+) \k<w>/.test("hello hello"), "named backref matches");
assert.isFalse(/(?<w>\w+) \k<w>/.test("hello world"), "named backref mismatch");
