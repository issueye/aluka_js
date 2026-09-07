# -*- coding: utf-8 -*-
# M1 专项 test262 风格用例生成器（一次性脚本）
import os

OUT = os.path.join("tests", "conformance", "test262", "cases")
cases = {}


def add(name, body, negative=None):
    fm = ""
    if negative:
        fm = "/*---\nnegative:\n  phase: runtime\n  type: %s\n---*/\n" % negative
    cases[name] = fm + body


# ============ Proxy / Reflect (45) ============
add("m1-proxy-001.js", 'assert.sameValue(typeof Proxy, "function", "Proxy is a function");\n')
add("m1-proxy-002.js", 'assert.sameValue(typeof Reflect, "object", "Reflect is an object");\n')
add("m1-proxy-003.js", '''
var t = { a: 1 };
var p = new Proxy(t, {});
assert.sameValue(p.a, 1, "get passthrough");
p.b = 2;
assert.sameValue(t.b, 2, "set passthrough");
assert.isTrue("a" in p, "has passthrough");
''')
add("m1-proxy-004.js", '''
var log = [];
var t = { x: 42 };
var p = new Proxy(t, { get: function (target, key) { log.push(key); return target[key]; } });
assert.sameValue(p.x, 42);
assert.sameValue(log.join(","), "x", "get trap fired with key");
''')
add("m1-proxy-005.js", '''
var seen = [];
var t = {};
var p = new Proxy(t, { get: function (target, key, receiver) { seen.push(key); return receiver === p ? "self" : "other"; } });
assert.sameValue(p.q, "self", "receiver is the proxy");
assert.sameValue(seen.join(","), "q");
''')
add("m1-proxy-006.js", '''
var writes = [];
var t = {};
var p = new Proxy(t, { set: function (target, key, value) { writes.push(key + "=" + value); target[key] = value; return true; } });
p.n = 7;
assert.sameValue(t.n, 7, "set trap forwarded");
assert.sameValue(writes.join(","), "n=7");
''')
add("m1-proxy-007.js", '''
var p = new Proxy({}, { set: function () { return false; } });
assert.throws(TypeError, function () { p.frozen = 1; });
''')
add("m1-proxy-008.js", '''
var hits = [];
var t = { a: 1 };
var p = new Proxy(t, { has: function (target, key) { hits.push(key); return key !== "a"; } });
assert.isTrue("b" in p, "has trap true path");
assert.isFalse("a" in p, "has trap false path");
assert.sameValue(hits.join(","), "b,a");
''')
add("m1-proxy-009.js", '''
var t = { gone: 1, keep: 2 };
var p = new Proxy(t, { deleteProperty: function (target, key) { return delete target[key]; } });
delete p.gone;
assert.sameValue(t.gone, undefined, "deleteProperty forwarded");
assert.sameValue(t.keep, 2, "other key untouched");
''')
add("m1-proxy-010.js", '''
var t = { b: 1, a: 2 };
var p = new Proxy(t, { ownKeys: function () { return ["a", "b"]; } });
assert.sameValue(Object.keys(p).join(","), "a,b", "ownKeys trap drives Object.keys");
''')
add("m1-proxy-011.js", '''
var t = { x: 1, y: 2 };
var p = new Proxy(t, { ownKeys: function (target) { return Object.keys(target).reverse(); } });
var acc = "";
for (var k in p) { acc += k; }
assert.sameValue(acc, "yx", "ownKeys trap drives for-in order");
''')
add("m1-proxy-012.js", '''
function fn(x) { return x * 2; }
var pf = new Proxy(fn, { apply: function (target, thisArg, args) { return Reflect.apply(target, thisArg, args) + 1; } });
assert.sameValue(pf(10), 21, "apply trap adds one");
''')
add("m1-proxy-013.js", '''
function Box(v) { this.v = v; }
var PB = new Proxy(Box, { construct: function (target, args) { var o = Reflect.construct(target, args); o.tag = "p"; return o; } });
var inst = new PB(9);
assert.sameValue(inst.v, 9, "construct trap forwards args");
assert.sameValue(inst.tag, "p", "construct trap extension");
assert.isTrue(inst instanceof Box, "instanceof Box");
''')
add("m1-proxy-014.js", '''
var proto = { marker: true };
var p = new Proxy({}, { getPrototypeOf: function () { return proto; } });
assert.isTrue(p.marker, "getPrototypeOf trap visible via chain");
var p2 = new Proxy({}, { getPrototypeOf: function () { return Array.prototype; } });
assert.isTrue(p2 instanceof Array, "trap-driven instanceof");
''')
add("m1-proxy-015.js", '''
var requested = false;
var target = {};
var p = new Proxy(target, {
  setPrototypeOf: function (t, proto) { requested = true; return Reflect.setPrototypeOf(t, proto); }
});
Reflect.setPrototypeOf(p, { z: 1 });
assert.isTrue(requested, "setPrototypeOf trap invoked");
assert.sameValue(p.z, 1, "prototype applied to target");
''')
add("m1-proxy-016.js", '''
var p = new Proxy({}, { isExtensible: function () { return true; } });
assert.isTrue(Reflect.isExtensible(p), "isExtensible trap");
''')
add("m1-proxy-017.js", '''
var asked = false;
var p = new Proxy({}, { preventExtensions: function (t) { asked = true; return Reflect.preventExtensions(t); } });
assert.isTrue(Reflect.preventExtensions(p), "preventExtensions trap returns true");
assert.isTrue(asked, "trap invoked");
''')
add("m1-proxy-018.js", '''
var p = new Proxy({ v: 5 }, { getOwnPropertyDescriptor: function (t, key) {
  var d = Reflect.getOwnPropertyDescriptor(t, key);
  if (d) { d.writable = false; }
  return d;
}});
var d = Reflect.getOwnPropertyDescriptor(p, "v");
assert.sameValue(d.writable, false, "trap-mutated descriptor");
assert.sameValue(d.value, 5, "descriptor value passthrough");
''')
add("m1-proxy-019.js", '''
var defined = [];
var t = {};
var p = new Proxy(t, { defineProperty: function (target, key, desc) { defined.push(key); return Reflect.defineProperty(target, key, desc); } });
Reflect.defineProperty(p, "q", { value: 3, writable: true, enumerable: true, configurable: true });
assert.sameValue(t.q, 3, "defineProperty forwarded");
assert.sameValue(defined.join(","), "q");
''')
add("m1-proxy-020.js", '''
var t = { a: 1 };
var p = new Proxy(t, {});
p.a = 9;
assert.sameValue(t.a, 9, "no-trap set forwards");
assert.sameValue(Object.keys(p).join(","), "a", "no-trap keys");
''')
add("m1-proxy-021.js", '''
var pair = Proxy.revocable({ x: 1 }, {});
assert.sameValue(pair.proxy.x, 1, "revocable works before revoke");
assert.sameValue(typeof pair.revoke, "function", "revoke is function");
pair.revoke();
assert.throws(TypeError, function () { pair.proxy.x; });
''')
add("m1-proxy-022.js", '''
var pair = Proxy.revocable({}, {});
pair.revoke();
assert.throws(TypeError, function () { pair.proxy.anything = 1; });
''')
add("m1-proxy-023.js", '''
function callable() { return 1; }
var p = new Proxy(callable, {});
assert.sameValue(typeof p, "function", "proxy of function is function");
var po = new Proxy({}, {});
assert.sameValue(typeof po, "object", "proxy of object is object");
''')
add("m1-proxy-024.js", '''
var t = { m: 1, n: 2 };
var p = new Proxy(t, {});
var copy = {};
for (var k in p) { copy[k] = t[k]; }
assert.sameValue(copy.m, 1, "for-in copy via proxy");
assert.sameValue(copy.n, 2);
''')
add("m1-proxy-025.js", '''
var arr = [10, 20, 30];
var p = new Proxy(arr, {});
assert.sameValue(p[1], 20, "array index through proxy");
assert.sameValue(p.length, 3, "length through proxy");
''')
add("m1-proxy-026.js", '''
var calls = 0;
var target = function () { calls += 1; return calls; };
var p = new Proxy(target, {});
p();
p();
assert.sameValue(calls, 2, "no-trap apply forwards");
''')
add("m1-proxy-027.js", '''
var t = { deep: { v: 1 } };
var p = new Proxy(t, {});
assert.sameValue(p.deep.v, 1, "nested get through proxy");
''')
add("m1-proxy-028.js", '''
var validate = {
  set: function (t, k, v) {
    if (k === "age" && typeof v !== "number") { throw new TypeError("age must be number"); }
    t[k] = v;
    return true;
  }
};
var person = new Proxy({}, validate);
person.age = 30;
assert.sameValue(person.age, 30);
assert.throws(TypeError, function () { person.age = "old"; });
''')
add("m1-proxy-029.js", '''
var defaults = new Proxy({}, {
  get: function (t, k) { return k in t ? t[k] : 0; }
});
defaults.known = 5;
assert.sameValue(defaults.known, 5, "explicit value");
assert.sameValue(defaults.missing, 0, "defaulted value");
''')
add("m1-proxy-030.js", '''
var log = [];
var t = { a: 1 };
var p = new Proxy(t, {
  get: function (t2, k) { log.push("get:" + k); return t2[k]; },
  set: function (t2, k, v) { log.push("set:" + k); t2[k] = v; return true; }
});
p.b = 2;
var v = p.a;
assert.sameValue(log.join(","), "set:b,get:a", "trap order recorded");
assert.sameValue(v, 1);
''')
add("m1-proxy-031.js", 'assert.sameValue(Reflect.get({ a: 7 }, "a"), 7, "Reflect.get basic");\n')
add("m1-proxy-032.js", '''
var t = { secret: 99 };
var p = new Proxy(t, { get: function (target, key) { return key === "secret" ? "***" : target[key]; } });
assert.sameValue(Reflect.get(p, "secret"), "***", "Reflect.get respects get trap");
''')
add("m1-proxy-033.js", '''
var t = {};
assert.isTrue(Reflect.set(t, "k", 5), "Reflect.set returns true");
assert.sameValue(t.k, 5);
''')
add("m1-proxy-034.js", '''
var t = { a: 1 };
assert.isTrue(Reflect.has(t, "a"), "Reflect.has present");
assert.isFalse(Reflect.has(t, "b"), "Reflect.has absent");
''')
add("m1-proxy-035.js", '''
var t = { doomed: 1 };
assert.isTrue(Reflect.deleteProperty(t, "doomed"));
assert.sameValue(t.doomed, undefined);
''')
add("m1-proxy-036.js", '''
var o = { x: 1, y: 2 };
var keys = Reflect.ownKeys(o);
assert.sameValue(keys.sort().join(","), "x,y", "Reflect.ownKeys keys");
''')
add("m1-proxy-037.js", '''
var d = Reflect.getOwnPropertyDescriptor({ v: 3 }, "v");
assert.sameValue(d.value, 3, "descriptor value");
assert.isTrue(d.writable && d.enumerable && d.configurable, "descriptor flags");
assert.sameValue(Reflect.getOwnPropertyDescriptor({}, "nope"), undefined, "missing descriptor");
''')
add("m1-proxy-038.js", '''
var base = { m: 1 };
var child = Object.create(base);
assert.sameValue(Reflect.getPrototypeOf(child), base, "getPrototypeOf chain");
''')
add("m1-proxy-039.js", '''
var t = {};
assert.isTrue(Reflect.setPrototypeOf(t, { tag: 1 }), "setPrototypeOf ok");
assert.sameValue(t.tag, 1, "proto applied");
''')
add("m1-proxy-040.js", '''
assert.isTrue(Reflect.isExtensible({}), "objects extensible");
assert.isTrue(Reflect.preventExtensions({}), "preventExtensions succeeds");
''')
add("m1-proxy-041.js", '''
var t = {};
assert.isTrue(Reflect.defineProperty(t, "x", { value: 8, writable: true, enumerable: true, configurable: true }));
assert.sameValue(t.x, 8);
''')
add("m1-proxy-042.js", '''
function sum(a, b) { return a + b; }
assert.sameValue(Reflect.apply(sum, null, [2, 3]), 5, "Reflect.apply");
''')
add("m1-proxy-043.js", '''
function Pt(v) { this.v = v; }
var inst = Reflect.construct(Pt, [4]);
assert.sameValue(inst.v, 4, "Reflect.construct");
assert.isTrue(inst instanceof Pt);
''')
add("m1-proxy-044.js", '''
var methods = ["apply", "construct", "defineProperty", "deleteProperty", "get",
  "getOwnPropertyDescriptor", "getPrototypeOf", "has", "isExtensible", "ownKeys",
  "preventExtensions", "set", "setPrototypeOf"];
var all = true;
for (var i = 0; i < methods.length; i++) {
  if (typeof Reflect[methods[i]] !== "function") { all = false; }
}
assert.isTrue(all, "all 13 Reflect methods are functions");
assert.sameValue(methods.length, 13, "13 methods listed");
''')
add("m1-proxy-045.js", '''
var t = { a: 1 };
var log = [];
var p = new Proxy(t, {
  get: function (t2, k) { log.push("get"); return Reflect.get(t2, k); },
  has: function (t2, k) { log.push("has"); return Reflect.has(t2, k); }
});
var g = p.a;
var h = "a" in p;
assert.sameValue(log.join(","), "get,has", "Reflect.* inside traps");
assert.sameValue(g, 1);
assert.isTrue(h);
''')

# ============ RegExp (12) ============
add("m1-regexp-001.js", '''
var m = /(?<=\\$)\\d+/.exec("price $100");
assert.sameValue(m[0], "100", "positive lookbehind");
assert.sameValue(m.index, 7, "match position after $");
''')
add("m1-regexp-002.js", '''
var m = /(?<!\\$)\\b\\d+/.exec("100 vs $20");
assert.sameValue(m[0], "100", "negative lookbehind skips $20");
''')
add("m1-regexp-003.js", '''
assert.isTrue(/(?<=x)y/.test("xy"), "lookbehind assertion true");
assert.isFalse(/(?<=x)y/.test("ay"), "lookbehind assertion false");
''')
add("m1-regexp-004.js", '''
var r = "2026-08-07".replace(/(?<=-)\\d+/g, "XX");
assert.sameValue(r, "2026-XX-XX", "lookbehind in replace");
''')
add("m1-regexp-005.js", '''
var m = /(?<year>\\d{4})-(?<month>\\d{2})/.exec("2026-08");
assert.sameValue(m.groups.year, "2026", "named group year");
assert.sameValue(m.groups.month, "08", "named group month");
''')
add("m1-regexp-006.js", '''
var r = "2026-08".replace(/(?<y>\\d{4})-(?<m>\\d{2})/, "$<m>/$<y>");
assert.sameValue(r, "08/2026", "named group replacement");
''')
add("m1-regexp-007.js", '''
assert.isTrue(/(?<w>\\w+) \\k<w>/.test("hello hello"), "named backref matches");
assert.isFalse(/(?<w>\\w+) \\k<w>/.test("hello world"), "named backref mismatch");
''')
add("m1-regexp-008.js", '''
var m = /(\\w+) \\1/.exec("say say again");
assert.sameValue(m[1], "say", "numeric backref capture");
assert.isTrue(/(a|b)\\1/.test("aa"), "backref aa");
assert.isFalse(/(a|b)\\1/.test("ab"), "backref ab");
''')
add("m1-regexp-009.js", '''
assert.isTrue(/\\bcat\\b/.test("a cat here"), "word boundary");
assert.isFalse(/\\bcat\\b/.test("concat"), "no boundary inside word");
''')
add("m1-regexp-010.js", '''
assert.isTrue(/\\Binside\\B/.test("kinsidez"), "non-boundary");
assert.isFalse(/\\Bcat/.test("cat"), "start is boundary");
''')
add("m1-regexp-011.js", '''
var re = /(?<a>x)(?<b>y)?/g;
var m = re.exec("zx");
assert.sameValue(m.groups.a, "x", "group a");
assert.sameValue(m.groups.b, undefined, "optional group undefined");
''')
add("m1-regexp-012.js", '''
var m = /(?<=(a+))b/.exec("aaab");
assert.sameValue(m[1], "aaa", "greedy lookbehind capture");
''')

# ============ ES2023-2024 (10) ============
add("m1-es2024-001.js", '''
var r = Promise.withResolvers();
assert.sameValue(typeof r.promise, "object", "promise present");
assert.sameValue(typeof r.resolve, "function", "resolve present");
assert.sameValue(typeof r.reject, "function", "reject present");
''')
add("m1-es2024-002.js", '''
var r = Promise.withResolvers();
r.resolve(42);
r.promise.then(function (v) { if (v !== 42) { throw new Error("bad resolve value " + v); } });
''')
add("m1-es2024-003.js", '''
var r = Promise.withResolvers();
r.reject("boom");
r.promise.catch(function (e) { if (e !== "boom") { throw new Error("bad reject"); } });
''')
add("m1-es2024-004.js", '''
var sorted = [3, 1, 2].toSorted();
assert.sameValue(sorted.join(","), "1,2,3", "toSorted sorts");
''')
add("m1-es2024-005.js", '''
var rev = [1, 2, 3].toReversed();
assert.sameValue(rev.join(","), "3,2,1", "toReversed");
''')
add("m1-es2024-006.js", '''
var sp = [1, 2, 3].toSpliced(1, 1, 9);
assert.sameValue(sp.join(","), "1,9,3", "toSpliced");
''')
add("m1-es2024-007.js", '''
var w = [1, 2, 3].with(1, 9);
assert.sameValue(w.join(","), "1,9,3", "with replaces");
''')
add("m1-es2024-008.js", '''
var base = [3, 1, 2];
base.toSorted();
base.toReversed();
assert.sameValue(base.join(","), "3,1,2", "immutable methods do not mutate");
''')
add("m1-es2024-009.js", '''
var g = Object.groupBy([1, 2, 3, 4], function (x) { return x % 2 === 0 ? "even" : "odd"; });
assert.sameValue(g.odd.join(","), "1,3", "Object.groupBy odd");
assert.sameValue(g.even.join(","), "2,4", "Object.groupBy even");
''')
add("m1-es2024-010.js", '''
var m = Map.groupBy(["a", "bb", "cc"], function (x) { return x.length; });
assert.sameValue(m.get(1), "a", "Map.groupBy single");
assert.sameValue(m.get(2), "bb,cc", "Map.groupBy pair");
assert.isTrue("abc".isWellFormed(), "isWellFormed true");
assert.sameValue("abc".toWellFormed(), "abc", "toWellFormed passthrough");
''')

# ============ TypedArray (15) ============
add("m1-typedarray-001.js", '''
var ab = new ArrayBuffer(16);
assert.sameValue(ab.byteLength, 16, "byteLength");
''')
add("m1-typedarray-002.js", '''
var i32 = new Int32Array(4);
i32[0] = 42;
i32[3] = -7;
assert.sameValue(i32[0], 42, "write read back");
assert.sameValue(i32[3], -7, "negative value");
assert.sameValue(i32.length, 4, "length");
''')
add("m1-typedarray-003.js", '''
var ab = new ArrayBuffer(8);
var i32 = new Int32Array(ab);
i32[0] = 1;
var u8 = new Uint8Array(ab);
assert.sameValue(u8[0], 1, "LE low byte");
assert.sameValue(u8[1], 0, "LE high bytes zero");
''')
add("m1-typedarray-004.js", '''
var c = new Uint8ClampedArray([-5, 128, 300, 3.7]);
assert.sameValue(c[0], 0, "clamp low");
assert.sameValue(c[1], 128, "in range");
assert.sameValue(c[2], 255, "clamp high");
assert.sameValue(c[3], 4, "rounds");
''')
add("m1-typedarray-005.js", '''
var f = new Float64Array([1.5, -2.25]);
assert.sameValue(f[0], 1.5, "float 1.5");
assert.sameValue(f[1], -2.25, "float -2.25");
''')
add("m1-typedarray-006.js", '''
var a = Int32Array.from([5, 6, 7]);
assert.sameValue(a.join(","), "5,6,7", "from");
var b = Uint8Array.of(9, 8);
assert.sameValue(b.join(","), "9,8", "of");
''')
add("m1-typedarray-007.js", '''
var i32 = new Int32Array(2);
assert.isTrue(Int32Array.isTypedArray(i32), "isTypedArray true");
assert.isFalse(Int32Array.isTypedArray([1, 2]), "plain array not TA");
assert.isTrue(ArrayBuffer.isView(i32), "TA is view");
assert.isFalse(ArrayBuffer.isView(new ArrayBuffer(4)), "buffer not view");
''')
add("m1-typedarray-008.js", '''
var i32 = new Int32Array([1, 2, 3, 4]);
var sub = i32.subarray(1, 3);
assert.sameValue(sub.length, 2, "subarray length");
sub[0] = 99;
assert.sameValue(i32[1], 99, "subarray shares buffer");
''')
add("m1-typedarray-009.js", '''
var i32 = new Int32Array([1, 2, 3]);
var cp = i32.slice(0, 2);
cp[0] = 77;
assert.sameValue(i32[0], 1, "slice copies");
assert.sameValue(cp.join(","), "77,2", "copied values");
''')
add("m1-typedarray-010.js", '''
var ab = new ArrayBuffer(4);
var dv = new DataView(ab);
dv.setInt32(0, 1, true);
var u8 = new Uint8Array(ab);
assert.sameValue(u8[0], 1, "LE byte order");
dv.setUint16(0, 4660, false);
assert.sameValue(dv.getUint16(0), 4660, "BE round trip");
assert.sameValue(dv.getUint16(0, true), 10244, "LE read of BE bytes");
''')
add("m1-typedarray-011.js", '''
var i32 = new Int32Array(3);
i32.set([7, 8]);
assert.sameValue(i32[0], 7, "set from array");
assert.sameValue(i32[1], 8);
i32.set([9], 2);
assert.sameValue(i32[2], 9, "set with offset");
''')
add("m1-typedarray-012.js", '''
var doubled = new Int32Array([1, 2, 3]).map(function (x) { return x * 2; });
assert.sameValue(doubled.join(","), "2,4,6", "typed map");
var evens = new Int32Array([1, 2, 3, 4]).filter(function (x) { return x % 2 === 0; });
assert.sameValue(evens.join(","), "2,4", "typed filter");
''')
add("m1-typedarray-013.js", '''
var s = new Int32Array([3, 1, 2]).sort();
assert.sameValue(s.join(","), "1,2,3", "numeric sort");
var r = new Int16Array([1, 2, 3]).reverse();
assert.sameValue(r.join(","), "3,2,1", "reverse in place");
''')
add("m1-typedarray-014.js", '''
var sum = 0;
var ta = new Int32Array([1, 2, 3]);
for (var v of ta) { sum += v; }
assert.sameValue(sum, 6, "for-of over typed array");
var keys = new Int32Array([9]).keys();
var step = keys.next();
assert.sameValue(step.value, 0, "keys iterator index");
assert.sameValue(step.done, false, "iterator not done");
''')
add("m1-typedarray-015.js", '''
var b = new BigInt64Array([9, -3]);
assert.sameValue(b[0].toString(), "9", "bigint read");
assert.sameValue(b[1].toString(), "-3", "negative bigint");
var u = new BigUint64Array([1]);
assert.sameValue(u[0].toString(), "1", "unsigned bigint");
''')

# negative tests
add("m1-negative-proxy-target.js", '''
var bad = new Proxy(42, {});
''', negative="TypeError")
add("m1-negative-regexp-group.js", '''
var re = new RegExp("(?<", "");
''', negative="SyntaxError")

# ============ eval / Function (10) ============
add("m1-eval-001.js", 'assert.sameValue(eval("1 + 2"), 3, "eval completion value");\n')
add("m1-eval-002.js", 'assert.sameValue(eval("40 + 2"), 42, "eval arithmetic");\n')
add("m1-eval-003.js", '''
function f() { var a = 10; var b = 20; return eval("a + b"); }
assert.sameValue(f(), 30, "direct eval reads enclosing scope");
''')
add("m1-eval-004.js", '''
function f() { var c = 1; eval("c = 99"); return c; }
assert.sameValue(f(), 99, "direct eval writes enclosing var");
''')
add("m1-eval-005.js", '''
var indirectEval = eval;
indirectEval("globalFromEval = 123");
assert.sameValue(globalFromEval, 123, "indirect eval writes global");
''')
add("m1-eval-006.js", '''
var f = new Function("return 5");
assert.sameValue(f(), 5, "new Function body");
''')
add("m1-eval-007.js", '''
var add = new Function("a", "b", "return a + b");
assert.sameValue(add(3, 4), 7, "new Function params");
''')
add("m1-eval-008.js", '''
var threw = false;
try { eval("function("); } catch (e) { threw = e instanceof SyntaxError; }
assert.isTrue(threw, "eval syntax error throws SyntaxError");
''')
add("m1-eval-009.js", '''
var threw = false;
try { new Function("return ((("); } catch (e) { threw = true; }
assert.isTrue(threw, "new Function invalid body throws");
''')
add("m1-eval-010.js", 'assert.sameValue(eval(), undefined, "eval of nothing");\n')

# array basics (补充语料覆盖)
add("m1-array-001.js", '''
var r = [3, 1, 2].reverse();
assert.sameValue(r.join(","), "2,1,3", "reverse in place");
''')
add("m1-array-002.js", '''
assert.sameValue([1, 2, 3].indexOf(2), 1, "indexOf");
assert.sameValue([1, 2, 3].indexOf(9), -1, "indexOf missing");
assert.isTrue([1, 2].includes(2), "includes");
assert.isFalse([1, 2].includes(3), "includes missing");
''')
add("m1-array-003.js", '''
assert.isTrue([1, 2, 3].every(function (x) { return x > 0; }), "every true");
assert.isFalse([1, -2].every(function (x) { return x > 0; }), "every false");
assert.isTrue([1, -2].some(function (x) { return x < 0; }), "some true");
''')
add("m1-array-004.js", '''
assert.sameValue([1, 2, 3].at(-1), 3, "at negative");
assert.sameValue([1, 2, 3].at(0), 1, "at zero");
''')
add("m1-array-005.js", '''
assert.sameValue([1, [2, 3]].flat().join(","), "1,2,3", "flat");
assert.sameValue([1, 2].flatMap(function (x) { return [x * 10]; }).join(","), "10,20", "flatMap");
''')
add("m1-array-006.js", '''
var a = [1, 2, 3, 4];
a.fill(9, 1, 3);
assert.sameValue(a.join(","), "1,9,9,4", "fill range");
var b = [1, 2, 3, 4];
b.copyWithin(0, 2);
assert.sameValue(b.join(","), "3,4,3,4", "copyWithin");
''')
add("m1-array-007.js", '''
var a = [1, 2, 3, 4];
var removed = a.splice(1, 2);
assert.sameValue(removed.join(","), "2,3", "splice removed");
assert.sameValue(a.join(","), "1,4", "splice remainder");
''')
add("m1-array-008.js", '''
assert.sameValue([1, 2].concat([3, 4]).join(","), "1,2,3,4", "concat arrays");
assert.sameValue([1].concat(2, [3]).join(","), "1,2,3", "concat mixed");
''')
add("m1-array-009.js", '''
var arr = [10, 20];
assert.sameValue(arr.toString(), "10,20", "array toString");
var it = arr.values();
var first = it.next();
assert.sameValue(first.value, 10, "values iterator");
assert.isFalse(first.done, "values not done");
''')
add("m1-array-010.js", '''
assert.sameValue([2, 1].findLast(function (x) { return x > 0; }), 1, "findLast");
assert.sameValue([1, 2, 3].findIndex(function (x) { return x === 2; }), 1, "findIndex");
assert.sameValue([1, 2, 3].findLastIndex(function (x) { return x > 1; }), 2, "findLastIndex");
''')

n = 0
for name, content in cases.items():
    path = os.path.join(OUT, name)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(content)
    n += 1
print("wrote", n, "cases; total in dir:", len(os.listdir(OUT)))
