
var p = new Proxy({}, { set: function () { return false; } });
assert.throws(TypeError, function () { p.frozen = 1; });
