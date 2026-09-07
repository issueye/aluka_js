
var defaults = new Proxy({}, {
  get: function (t, k) { return k in t ? t[k] : 0; }
});
defaults.known = 5;
assert.sameValue(defaults.known, 5, "explicit value");
assert.sameValue(defaults.missing, 0, "defaulted value");
