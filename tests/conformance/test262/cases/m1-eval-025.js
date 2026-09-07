
var out = eval("var acc = []; acc.push(1); acc.push(2); acc.join('-')");
assert.sameValue(out, "1-2", "method chain on var");
