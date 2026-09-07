
var r = Promise.withResolvers();
r.resolve(42);
r.promise.then(function (v) { if (v !== 42) { throw new Error("bad resolve value " + v); } });
