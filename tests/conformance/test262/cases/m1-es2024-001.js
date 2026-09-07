
var r = Promise.withResolvers();
assert.sameValue(typeof r.promise, "object", "promise present");
assert.sameValue(typeof r.resolve, "function", "resolve present");
assert.sameValue(typeof r.reject, "function", "reject present");
