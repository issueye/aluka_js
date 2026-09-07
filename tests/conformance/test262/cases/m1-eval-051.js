
assert.sameValue(eval("  "), undefined, "whitespace only");
assert.sameValue(eval("// comment"), undefined, "comment only");
