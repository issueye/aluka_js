
var c = new Uint8ClampedArray([-5, 128, 300, 3.7]);
assert.sameValue(c[0], 0, "clamp low");
assert.sameValue(c[1], 128, "in range");
assert.sameValue(c[2], 255, "clamp high");
assert.sameValue(c[3], 4, "rounds");
