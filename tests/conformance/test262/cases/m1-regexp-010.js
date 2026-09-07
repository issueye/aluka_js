
assert.isTrue(/\Binside\B/.test("kinsidez"), "non-boundary");
assert.isFalse(/\Bcat/.test("cat"), "start is boundary");
