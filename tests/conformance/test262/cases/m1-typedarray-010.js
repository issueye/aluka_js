
var ab = new ArrayBuffer(4);
var dv = new DataView(ab);
dv.setInt32(0, 1, true);
var u8 = new Uint8Array(ab);
assert.sameValue(u8[0], 1, "LE byte order");
dv.setUint16(0, 4660, false);
assert.sameValue(dv.getUint16(0), 4660, "BE round trip");
assert.sameValue(dv.getUint16(0, true), 13330, "LE read of BE bytes");
