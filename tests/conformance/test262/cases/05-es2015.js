/*---
esid: local-es2015
description: ES2015 特性（let/const/箭头/class/模板字符串）
---*/
// let / const / 块级作用域。
let a = 1;
const B = 2;
{
  let a = 10;
  assert.sameValue(a, 10, "block scoped let");
}
assert.sameValue(a, 1, "outer let");
assert.sameValue(B, 2, "const");

// 箭头函数。
var add = (x, y) => x + y;
assert.sameValue(add(3, 4), 7, "arrow function");

// 模板字符串。
var name = "aluka";
assert.sameValue(`hello ${name}`, "hello aluka", "template literal");

// class。
class Point {
  constructor(x, y) { this.x = x; this.y = y; }
  dist() { return this.x + this.y; }
}
class Point3D extends Point {
  constructor(x, y, z) { super(x, y); this.z = z; }
  dist() { return super.dist() + this.z; }
}
var p = new Point3D(1, 2, 3);
assert.sameValue(p.dist(), 6, "class inheritance + super");

// 解构。
var [first, second] = [10, 20];
assert.sameValue(first + second, 30, "array destructuring");
var { x, y } = { x: 5, y: 6 };
assert.sameValue(x + y, 11, "object destructuring");

// 默认参数 / rest。
function greet(name, greeting = "hi") { return greeting + " " + name; }
assert.sameValue(greet("al"), "hi al", "default param");
function sumAll(...nums) { return nums.reduce((a, b) => a + b, 0); }
assert.sameValue(sumAll(1, 2, 3, 4), 10, "rest params");

// Map / Set。
var m = new Map();
m.set("k", "v");
assert.sameValue(m.get("k"), "v", "Map");
var s = new Set([1, 2, 2, 3]);
assert.sameValue(s.size, 3, "Set dedup");

// Symbol。
var sym = Symbol("x");
assert.sameValue(typeof sym, "symbol", "Symbol");

// Promise（异步由 harness 外验证，这里测 thenable）。
var pr = Promise.resolve(7);
pr.then(function(v) { assert.sameValue(v, 7, "Promise.resolve"); });
