
var re = /(?<a>x)(?<b>y)?/g;
var m = re.exec("zx");
assert.sameValue(m.groups.a, "x", "group a");
assert.sameValue(m.groups.b, undefined, "optional group undefined");
