
assert.isTrue(/(?<=x)y/.test("xy"), "lookbehind assertion true");
assert.isFalse(/(?<=x)y/.test("ay"), "lookbehind assertion false");
