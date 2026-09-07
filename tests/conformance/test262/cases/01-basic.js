/*---
esid: local-basic
description: 基础表达式与变量（ES5）
---*/
var x = 1;
var y = 2;
assert.sameValue(x + y, 3, "arithmetic");
assert.sameValue("a" + "b", "ab", "string concat");
assert.sameValue(7 % 3, 1, "modulo");
assert.sameValue(2 * 3 + 4, 10, "precedence");
assert.isTrue(1 < 2, "less than");
assert.isFalse(3 <= 2, "less equal");

var arr = [1, 2, 3];
assert.sameValue(arr.length, 3, "array length");
assert.sameValue(arr[1], 2, "array index");
arr[3] = 4;
assert.sameValue(arr.length, 4, "array grow");

var obj = { a: 1, b: 2 };
assert.sameValue(obj.a + obj.b, 3, "object props");
obj.c = 3;
assert.sameValue(obj.c, 3, "object add");

assert.isTrue(typeof 1 === "number", "typeof number");
assert.isTrue(typeof "s" === "string", "typeof string");
assert.isTrue(typeof {} === "object", "typeof object");
