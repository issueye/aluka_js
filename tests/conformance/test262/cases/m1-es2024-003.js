
var r = Promise.withResolvers();
r.reject("boom");
r.promise.catch(function (e) { if (e !== "boom") { throw new Error("bad reject"); } });
