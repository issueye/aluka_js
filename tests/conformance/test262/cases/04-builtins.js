/*---
esid: local-builtins
description: 内置对象与方法
---*/
// Array 方法。
var mapped = [1, 2, 3].map(function(n) { return n * 2; });
assert.sameValue(mapped.join(","), "2,4,6", "map");

var filtered = [1, 2, 3, 4].filter(function(n) { return n % 2 === 0; });
assert.sameValue(filtered.join(","), "2,4", "filter");

var sum = [1, 2, 3].reduce(function(a, b) { return a + b; }, 0);
assert.sameValue(sum, 6, "reduce");

// String 方法。
assert.sameValue("hello".toUpperCase(), "HELLO", "toUpperCase");
assert.sameValue("hello".indexOf("ll"), 2, "indexOf");
assert.sameValue("a,b,c".split(",").length, 3, "split");

// Object 方法。
var keys = Object.keys({ a: 1, b: 2 });
assert.sameValue(keys.length, 2, "Object.keys");
assert.isTrue(Object.prototype.hasOwnProperty.call({ x: 1 }, "x"), "hasOwnProperty");

// Math。
assert.sameValue(Math.max(1, 5, 3), 5, "Math.max");
assert.sameValue(Math.floor(3.7), 3, "Math.floor");
assert.sameValue(Math.abs(-4), 4, "Math.abs");

// JSON。
var parsed = JSON.parse('{"k": 42}');
assert.sameValue(parsed.k, 42, "JSON.parse");
assert.sameValue(JSON.stringify({ a: 1 }), '{"a":1}', "JSON.stringify");
