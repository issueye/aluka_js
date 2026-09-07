# -*- coding: utf-8 -*-
# M1.6 eval/Function 专项用例扩充：10 → ≥50
import os

OUT = os.path.join("tests", "conformance", "test262", "cases")
cases = {}


def add(name, body, negative=None):
    fm = ""
    if negative:
        fm = "/*---\nnegative:\n  phase: runtime\n  type: %s\n---*/\n" % negative
    cases[name] = fm + body


# ===== 完成值（表达式形态）=====
add("m1-eval-011.js", 'assert.sameValue(eval("42"), 42, "numeric literal");\n')
add("m1-eval-012.js", 'assert.sameValue(eval("\'text\'"), "text", "string literal");\n')
add("m1-eval-013.js", 'assert.isTrue(eval("true"), "boolean literal");\n')
add("m1-eval-014.js", 'assert.sameValue(eval("2 + 3 * 4"), 14, "operator precedence");\n')
add("m1-eval-015.js", 'assert.sameValue(eval("[1,2,3].length"), 3, "member expression");\n')
add("m1-eval-016.js", 'assert.sameValue(eval("({ v: 9 }).v"), 9, "object literal member");\n')
add("m1-eval-017.js", 'assert.sameValue(eval("1 ? \'y\' : \'n\'"), "y", "conditional expression");\n')
add("m1-eval-018.js", 'assert.sameValue(eval("null ?? \'dflt\'"), "dflt", "nullish coalescing");\n')
add("m1-eval-019.js", 'assert.sameValue(eval("JSON.parse(\'{\\"k\\":2}\').k"), 2, "JSON inside eval");\n')
add("m1-eval-020.js", 'assert.sameValue(eval("Math.max(3, 9, 1)"), 9, "builtin global in eval");\n')

# ===== 多语句 var 声明与完成值 =====
add("m1-eval-021.js", 'assert.sameValue(eval("var a = 2; var b = 3; a * b"), 6, "multi var then expr");\n')
add("m1-eval-022.js", 'assert.sameValue(eval("var s = 0; s + 1"), 1, "var then read");\n')
add("m1-eval-023.js", 'assert.sameValue(eval("1; 2; 3"), 3, "sequence of expressions");\n')
add("m1-eval-024.js", '''
var r = eval("var sum = 0; sum += 1; sum += 2; sum");
assert.sameValue(r, 3, "compound assignment sequence");
''')
add("m1-eval-025.js", '''
var out = eval("var acc = []; acc.push(1); acc.push(2); acc.join('-')");
assert.sameValue(out, "1-2", "method chain on var");
''')
add("m1-eval-026.js", '''
var cnt = eval("var c = 0; for (var i = 0; i < 4; i++) { c += 1; } c");
assert.sameValue(cnt, 4, "for loop with counter var");
''')
add("m1-eval-027.js", '''
var total = eval("var t = 0; var list = [1, 2, 3]; for (var idx = 0; idx < list.length; idx++) { t += list[idx]; } t");
assert.sameValue(total, 6, "loop over array");
''')
add("m1-eval-028.js", '''
var picked = eval("var src = [10, 20, 30]; src.filter(function (x) { return x > 15; }).join(',')");
assert.sameValue(picked, "20,30", "filter inside eval");
''')

# ===== 直接求值词法穿透 =====
add("m1-eval-029.js", '''
function f() { var x = 7; return eval("x + 1"); }
assert.sameValue(f(), 8, "reads var");
''')
add("m1-eval-030.js", '''
function f(a, b) { return eval("a - b"); }
assert.sameValue(f(9, 4), 5, "reads parameters");
''')
add("m1-eval-031.js", '''
function f() { var n = 1; eval("n = n + 41"); return n; }
assert.sameValue(f(), 42, "writes var");
''')
add("m1-eval-032.js", '''
function f() { var arr = [1, 2]; eval("arr.push(3)"); return arr.join(","); }
assert.sameValue(f(), "1,2,3", "mutates object through eval");
''')
add("m1-eval-033.js", '''
function outer() {
  var secret = 5;
  function inner() { return eval("secret * 2"); }
  return inner();
}
assert.sameValue(outer(), 10, "upvalue penetration in nested fn");
''')
add("m1-eval-034.js", '''
function outer() {
  var counter = 0;
  var step = function () { eval("counter += 2"); };
  step();
  step();
  return counter;
}
assert.sameValue(outer(), 4, "eval writes via shared upvalue");
''')
add("m1-eval-035.js", '''
function make() { var base = 100; return function (d) { return eval("base - d"); }; }
assert.sameValue(make()(15), 85, "closure factory with eval");
''')

# ===== 间接求值与全局 =====
add("m1-eval-036.js", '''
var ie = eval;
ie("globalOne = 111");
assert.sameValue(globalOne, 111, "indirect eval creates global");
''')
add("m1-eval-037.js", '''
globalTwo = 20;
assert.sameValue(eval("globalTwo * 2"), 40, "reads global var");
''')
add("m1-eval-038.js", '''
var holder = { v: 1 };
eval("holder.v = 2");
assert.sameValue(holder.v, 2, "mutates outer object");
''')
add("m1-eval-039.js", 'assert.sameValue(typeof eval, "function", "eval is a function");\n')

# ===== new Function =====
add("m1-eval-040.js", '''
var f = new Function("");
assert.sameValue(f(), undefined, "empty body returns undefined");
''')
add("m1-eval-041.js", '''
var f = new Function("a", "b", "c", "return a + b + c;");
assert.sameValue(f(1, 2, 3), 6, "three params");
assert.sameValue(f.length, 3, "Function.length reflects params");
''')
add("m1-eval-042.js", '''
var f = Function("return 8;");
assert.sameValue(f(), 8, "callable without new");
''')
add("m1-eval-043.js", '''
var adder = new Function("x", "return function (y) { return x + y; };");
var add5 = adder(5);
assert.sameValue(add5(3), 8, "currying via Function constructor");
''')
add("m1-eval-044.js", '''
var f = new Function("return [1, 2, 3].reduce(function (a, b) { return a + b; }, 0);");
assert.sameValue(f(), 6, "array ops in body");
''')
add("m1-eval-045.js", '''
var f = new Function("n", "var r = 1; for (var i = 1; i <= n; i++) { r *= i; } return r;");
assert.sameValue(f(5), 120, "loop in body computes factorial");
''')
add("m1-eval-046.js", '''
var f = new Function("return Math.floor(4.7);");
assert.sameValue(f(), 4, "builtin global in body");
''')
add("m1-eval-047.js", '''
var greet = new Function("name", "return 'hello ' + name;");
assert.sameValue(greet("world"), "hello world", "string concat in body");
''')

# ===== eval 返回函数 / 嵌套 eval =====
add("m1-eval-048.js", '''
var mk = eval("(function (a) { return function (b) { return a + b; }; })");
assert.sameValue(mk(1)(2), 3, "eval returns closure");
''')
add("m1-eval-049.js", 'assert.sameValue(eval("eval(\'5\') + 1"), 6, "nested eval");\n')
add("m1-eval-050.js", '''
function f() { eval("function declared() { return 77; }"); return declared(); }
assert.sameValue(f(), 77, "function declaration inside eval callable");
''')

# ===== 错误与边界 =====
add("m1-eval-051.js", '''
assert.sameValue(eval("  "), undefined, "whitespace only");
assert.sameValue(eval("// comment"), undefined, "comment only");
''')
add("m1-eval-052.js", '''
var r = eval("try { nonexistentFn(); } catch (e) { 'recovered'; }");
assert.sameValue(r, "recovered", "try/catch inside eval");
''')

n = 0
for name, content in cases.items():
    path = os.path.join(OUT, name)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(content)
    n += 1
print("wrote", n, "cases")
