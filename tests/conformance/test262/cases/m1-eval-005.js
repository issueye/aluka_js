
var indirectEval = eval;
indirectEval("globalFromEval = 123");
assert.sameValue(globalFromEval, 123, "indirect eval writes global");
