
var holder = { v: 1 };
eval("holder.v = 2");
assert.sameValue(holder.v, 2, "mutates outer object");
