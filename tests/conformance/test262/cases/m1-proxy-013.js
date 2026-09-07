
function Box(v) { this.v = v; }
var PB = new Proxy(Box, { construct: function (target, args) { var o = Reflect.construct(target, args); o.tag = "p"; return o; } });
var inst = new PB(9);
assert.sameValue(inst.v, 9, "construct trap forwards args");
assert.sameValue(inst.tag, "p", "construct trap extension");
assert.isTrue(inst instanceof Box, "instanceof Box");
