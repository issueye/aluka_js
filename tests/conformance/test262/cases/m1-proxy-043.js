
function Pt(v) { this.v = v; }
var inst = Reflect.construct(Pt, [4]);
assert.sameValue(inst.v, 4, "Reflect.construct");
assert.isTrue(inst instanceof Pt);
