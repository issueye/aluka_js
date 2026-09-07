
var b = new BigInt64Array([9, -3]);
assert.sameValue(b[0].toString(), "9", "bigint read");
assert.sameValue(b[1].toString(), "-3", "negative bigint");
var u = new BigUint64Array([1]);
assert.sameValue(u[0].toString(), "1", "unsigned bigint");
