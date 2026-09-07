
var m = /(\w+) \1/.exec("say say again");
assert.sameValue(m[1], "say", "numeric backref capture");
assert.isTrue(/(a|b)\1/.test("aa"), "backref aa");
assert.isFalse(/(a|b)\1/.test("ab"), "backref ab");
