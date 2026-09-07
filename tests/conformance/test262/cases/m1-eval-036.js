
var ie = eval;
ie("globalOne = 111");
assert.sameValue(globalOne, 111, "indirect eval creates global");
