// O-5 回归：arguments 对象语义（去每帧 arguments 创建优化后必须保持）。
// 普通用例：node 与 aluka 直接运行逐行对比。
function f(a) { return arguments.length + ":" + arguments[0] + ":" + arguments[1]; }
console.log("R1>" + f(1, 2, 3));

function outer(a) { return (() => arguments.length + ":" + arguments[0])(); }
console.log("R2>" + outer(9, 8));

function dflt(a = 5) { return arguments.length + ":" + a; }
console.log("R3>" + dflt());

function rest(...r) { return arguments.length + ":" + r.join(","); }
console.log("R4>" + rest(1, 2, 3));

function nest() { function inner() { return arguments.length; } return inner(7); }
console.log("R5>" + nest());

function cond(x) { if (x) { return arguments.length; } return arguments[0]; }
console.log("R6>" + cond(1) + "|" + cond(0));

function w() { arguments.foo = 1; return arguments.foo; }
console.log("R7>" + w());

function make(a) { return function () { return arguments.length; }; }
console.log("R8>" + make(1)(2, 3));
