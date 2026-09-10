import { mkdirSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
// 工具位于 tools/，语料产出到 ../cases/gen/（runner 只扫描 cases/ 树）
const target = join(here, '..', 'cases', 'gen');

// —— 表达式矩阵：每条产出一个用例 ————————————————————————————————

const E = [];

// lang-core：运算符与强制转换
const langCore = [
  '1 + 1', '1 + "1"', '"1" + 1', '"a" + 1', '1 - 1', '"5" * "2"', '"5" - 2',
  '10 / 4', '10 % 3', '-10 % 3', '2 ** 10', '0.1 + 0.2', '1 / 0', '-1 / 0', '0 / 0',
  '5 & 3', '5 | 3', '5 ^ 3', '~5', '1 << 4', '-16 >> 2', '-16 >>> 28',
  '!!"a"', '!0', 'Boolean("")', 'Boolean({})', 'Number("")', 'Number(" 42 ")', 'Number("4x")',
  'Number(null)', 'Number(undefined)', 'Number(true)', 'Number([])', 'Number([7])', 'Number([1,2])',
  'String(null)', 'String(undefined)', 'String(-0)', 'String(1e21)', 'String(0.000001)',
  '+"-42"', '+"" ', '+null', '+undefined', '+{}', '+[3]',
  'parseInt("42px")', 'parseFloat("3.5rem")', 'parseInt("0x10")', 'parseInt("08")',
  '(123.456).toFixed(2)', '(255).toString(16)', '(8).toString(2)', '(-8).toString(2)',
  'Number.EPSILON', 'Number.MAX_SAFE_INTEGER + 1', 'Number.isInteger(4.0)',
  'void 0', 'typeof (() => {})', 'typeof class {}', 'typeof null', 'typeof NaN',
  '[] + []', '[] + {}', '{} + 0', 'true + true', 'null + 1', 'undefined + 1',
  '3 > 2 > 1', '"b" > "a"', 'NaN === NaN', '0 === -0', 'Object.is(NaN, NaN)', 'Object.is(0, -0)',
  '[1,2,3].toString()', '(function(){ return arguments.length })(1,2,3)',
  '(({ x: 1 })).x', '({}).toString.call([])',
  'delete ({a:1}).a', 'void ({}) instanceof Object',
  '1 ? "y" : "n"', '"" ? "y" : "n"',
  'a?.b?.c', 'undefined ?? "dflt"', '0 ?? "dflt"', 'null ?? "dflt"',
];
for (const expr of langCore) E.push(['lang-core', expr]);

// string
const strings = [
  '"abc".length', '"abc".charAt(1)', '"abc"[1]', '"abc".at(-1)',
  '"a-b-c".split("-")', '"  x  ".trim()', '"ab".repeat(3)',
  '"Hello".toLowerCase()', '"Hello".toUpperCase()',
  '"abc".indexOf("b")', '"abc".indexOf("z")', '"aaa".replaceAll("a", "b")',
  '"abc".slice(1)', '"abc".slice(-2)', '"abc".substring(1, 2)',
  '"abc".padStart(5, "*")', '"5".padStart(2, "0")', '"abc".padEnd(5)',
  '"aBc".startsWith("a")', '"aBc".endsWith("c")', '"aBc".includes("B")',
  '"a".codePointAt(0)', 'String.fromCodePoint(97)',
  '"x".charCodeAt(0)', '"\\u00e9".normalize("NFD").length',
  '"abc".concat("def")', '"abc".match(/b/)[0]',
  '"a1b2".replace(/\\d/g, "#")', '"a1b2".replace(/\\d/, "#")',
  '"one two".replace(/(\\w+) (\\w+)/, "$2 $1")',
  'String.raw`a\nb`', '"abc".indexOf("c")',
  '"".split(",")', '"a,".split(",")',
  'JSON.stringify("tab\\there")', '"\\ud83d\\ude00".length', '"\\ud83d\\ude00".codePointAt(0)',
];
for (const expr of strings) E.push(['string', expr]);

// array
const arrays = [
  '[3,1,2].sort()', '[3,1,2].sort((a,b)=>a-b)', '[1,2,3].reverse()',
  '[1,2,3].map(x=>x*2)', '[1,2,3].filter(x=>x>1)', '[1,2,3].reduce((a,b)=>a+b, 0)',
  '[1,2,3].find(x=>x>1)', '[1,2,3].findIndex(x=>x>1)', '[1,2,3].findLast(x=>x<3)',
  '[1,2,3].includes(2)', '[1,2,3].indexOf(3)', '[1,2,3].flat()',
  '[1,[2,[3]]].flat(2)', '[1,2].concat([3],[4])', '[1,2,3].slice(1)',
  '[1,2,3].join("-")', 'Array.from("abc")', 'Array.from({length:3},(_,i)=>i*i)',
  'Array(3).fill(7)', 'Array.of(1,2)', '[...[1,2], ...[3]]',
  '[1,2,3].at(-1)', '[].at(0)',
  '[1,2,3].every(x=>x>0)', '[1,2,3].some(x=>x>2)',
  '[1,2,3].flatMap(x=>[x,x])', '[5,1,5].lastIndexOf(5)',
  '[1,2,3].keys().next().value', '[..."ab"]',
  '[10,20].entries().next().value',
  '[1,2,3].reduceRight((a,b)=>""+a+b)',
  '[1,2,3].toSorted((a,b)=>b-a)', '[1,2,3].toReversed()', '[1,2,3].with(1, 99)',
  '[1,2,3].copyWithin(0, 2)',
  'JSON.stringify([1, null, "x"])',
'[1,2,3].pop()', '[1,2].push(3)',
  '[1,2,3].shift()', '[1,2].unshift(0)', '[1,2,3].splice(1,1,"x")',
];
for (const expr of arrays) E.push(['array', expr]);

// math-number
const maths = [
  'Math.abs(-3.5)', 'Math.ceil(1.2)', 'Math.floor(1.8)', 'Math.round(2.5)', 'Math.round(-2.5)',
  'Math.trunc(-3.9)', 'Math.max(1,5,3)', 'Math.min(-1,-5)', 'Math.sqrt(16)', 'Math.cbrt(27)',
  'Math.pow(2, 8)', 'Math.sign(-9)', 'Math.hypot(3,4)', 'Math.log2(8)', 'Math.log10(1000)',
  'Math.sin(0)', 'Math.cos(0)', 'Math.atan2(1, 1)',
  'Math.random() >= 0', 'Math.floor(Math.random() * 1)',
  'Math.max()', 'Math.min()', 'Math.max(Infinity)', 'Math.abs(-0)',
  '(0.1).toFixed(20) === "0.10000000000000000555"',
  '9007199254740993', '2 ** 53', '2 ** 53 + 0.5', '-(2 ** 53)',
  'Number("1e2")', 'Number("0b101")', 'Number("0o17")', 'Number("0xF")',
  '(1.5).toPrecision(2)', '(1234.5).toExponential(2)',
  'Number.isFinite(1/0)', 'Number.isNaN("NaN")', 'isFinite("5")',
];
for (const expr of maths) E.push(['math-number', expr]);

// object-json
const objs = [
  'Object.keys({a:1,b:2})', 'Object.values({a:1,b:2})', 'Object.entries({a:1})',
  'Object.assign({}, {a:1}, {b:2})', 'Object.freeze({}) && 1',
  'Object.getOwnPropertyNames("ab")',
  'JSON.stringify({b:1,a:2})', 'JSON.stringify([1,undefined,2])',
  'JSON.stringify({u:undefined,f:()=>1})', 'JSON.stringify({toJSON:()=>"j"})',
  'JSON.parse(\'{"a":[1,{"b":null}]}\')',
  'JSON.parse("1.5")', 'JSON.stringify(1e300)',
  'Object.fromEntries([["a",1],["b",2]])',
  '({a:1, ...rest})', '({...null, x:1})',
  'Object.hasOwn({a:1}, "a")', 'Object.hasOwn({a:1}, "b")',
  '"a" in {a:1}', '"toString" in {a:1}',
  'JSON.stringify({n: 1e-7})',
  'Object.getOwnPropertyDescriptor([7], "length").value',
  'Object.is(Object.freeze({}), Object.freeze({}))',
  'structuredClone({a:[1,{b:"c"}]})',
  'typeof structuredClone(new Map([["k",1]]))',
];
for (const expr of objs) E.push(['object-json', expr]);

// map-set
const mapset = [
  '[...new Map([[1,"a"],[2,"b"]]).entries()]',
  '[...new Set([1,2,2,3])]',
  'new Map([["a",1]]).get("a")', 'new Map([["a",1]]).has("b")',
  'new Set([1,2]).has(2)', 'new Set([1,2]).size',
  '(() => { const m = new Map(); m.set("k", 7); m.set("k", 8); return m.get("k") })()',
  'new Map([[1,"a"]]).delete(1)',
  '[...new Set("abca")]',
  'new Map([[{},{1:1}]]).size',
  'new Set([NaN, NaN]).size',
  '(() => { const s = new Set([1,2,3]); s.delete(2); return [...s] })()',
  'Map.groupBy([{t:"a"},{t:"b"},{t:"a"}], x=>x.t).size',
  'Object.groupBy([1,2,3,4], x=>x%2 ? "odd" : "even").odd.length',
  '[...new Map([[1,[2,3]]]).values()][0].length',
];
for (const expr of mapset) E.push(['map-set', expr]);

// regexp
const regexps = [
  '"a1b2".match(/\\d/g)', '"Aaa".match(/a/)', '"Aaa".match(/a/i)',
  /(\w+)-(\w+)/.exec('ab-cd').slice(0),
  '"2024-01-02".replace(/(\\d+)-(\\d+)-(\\d+)/, "$3/$2/$1")',
  '"aaa".split(/a/)',
  '"abc".search(/b/)',
  /a(?=b)/.test("ab"), /a(?!b)/.test("ab"),
  /(?<=x)y/.test("xy"), /(?<!x)y/.test("xy"),
  '"aaa".match(/a*/)',
  /\\s/.test(" "),
  'String(/ab/g)',
  '"a.b".replace(/\\./, "-")',
  'new RegExp("a\\\\d", "g").test("a1")',
  '"x".match(/(x)\\1/)',
];
for (const expr of regexps) E.push(['regexp', expr]);

// promise-async（确定性时序）
const promiseCases = [
  'Promise.resolve(1).then(x => x + 1)',
  '(async () => 40 + 2)()',
  '(async () => { const v = await Promise.resolve("v"); return v + "!" })()',
  'Promise.all([Promise.resolve(1), Promise.resolve(2)])',
  'Promise.allSettled([Promise.reject("x"), Promise.resolve(1)])',
  'Promise.race([Promise.resolve("first"), new Promise(()=>{})])',
  'Promise.resolve("z").finally(() => 0)',
  '(async () => { try { await Promise.reject(new Error("E")); } catch (e) { return e.name } })()',
  'Promise.any([Promise.reject("a"), Promise.resolve("b")])',
  '(async () => { for (const v of [1,2]) { await Promise.resolve(v) } return "loop-done" })()',
];
for (const expr of promiseCases) E.push(['promise-async', expr]);

// error
const errors = [
  '(() => { try { null.x } catch (e) { return e.name } })()',
  '(() => { try { undefined() } catch (e) { return e.name } })()',
  '(() => { try { JSON.parse("{") } catch (e) { return e.name } })()',
  '(() => { try { (1).toFixed(101) } catch (e) { return e.name } })()',
  '(() => { try { decodeURIComponent("%") } catch (e) { return e.name } })()',
  '(() => { try { throw new TypeError("T"); } catch (e) { return e.message } })()',
  'new RangeError("R").name',
  '(() => { try { const s = new Set(); s.add(); } catch (e) { return "thrown" } })()',
  '(() => { try { return (function(){ return this === undefined }).call(undefined) } finally {} })()',
];
for (const expr of errors) E.push(['error', expr]);

// class-proto
const classes = [
  '(() => { class A { m() { return "am" } } return new A().m() })()',
  '(() => { class A {} class B extends A {} return new B() instanceof A })()',
  '(() => { class P { get v() { return 9 } } return new P().v })()',
  '(() => { class C { static s() { return "st" } } return C.s() })()',
  '(() => { class C { #p = 5; get() { return this.#p } } return new C().get() })()',
  '(() => { class A { constructor() { this.x = 1 } } class B extends A { constructor() { super(); this.y = 2 } } const b = new B(); return [b.x, b.y] })()',
  'Object.getPrototypeOf([]) === Array.prototype',
  '(() => { function F() {} F.prototype.m = 1; const f = new F(); return f.m })()',
  '(() => { class C {} return typeof C.prototype.constructor })()',
  '(() => { class A { static { A.TAG = "t" } } return A.TAG })()',
];
for (const expr of classes) E.push(['class-proto', expr]);

// generators/destructuring/spread 等
const langMore = [
  '(() => { function* g(){ yield 1; yield 2 } return [...g()] })()',
  '(() => { const {a, b = 9} = {a:1}; return [a, b] })()',
  '(() => { const [x, , z] = [1,2,3]; return [x, z] })()',
  '(() => { const f = (a, ...r) => r; return f(1,2,3) })()',
  '(() => { let s = 0; for (const x of [1,2,3]) s += x; return s })()',
  '(() => { const o = {p:1, q:2}; let ks = ""; for (const k in o) ks += k; return ks })()',
  '(() => { const t = `a${1+1}b`; return t })()',
  '(() => { const [a = 4] = []; return a })()',
  '(() => { const o1 = {v:1}; const o2 = {v:2}; const c = {...o1, ...o2}; return c.v })()',
  '(() => { function f(a = Math.floor(1.9)) { return a } return f() })()',
  '(() => { const m = new Map(); m.set(m, "self"); return m.size })()',
  'eval("6 * 7")',
  '(() => { "use strict"; let n = 0; const inc = () => ++n; inc(); return inc() })()',
  '(() => { let x = 10; { let x = 20; } return x })()',
  '(() => { const o = {}; o["computed"] = 3; return o.computed })()',
  '(() => { function* g(){ const x = yield 1; yield x * 2 } const it = g(); it.next(); return it.next(5).value })()',
  '(() => { const arr = [1,2,3]; arr[5] = 9; return arr.length })()',
  '(() => { return [..."\\ud83d\\ude00"].length })()',
  '(() => { const v = 1; switch (v) { case 1: return "one" } })()',
  '(() => { const o = new Proxy({}, { get: () => 42 }); return o.anything })()',
  '(() => { const t = []; t.length = 0; return t.push(1) })()',
  '(() => { try { return "try" } catch { return "catch" } finally { } })()',
  '(() => { let c = 0; do { c++ } while (c < 3); return c })()',
  '(() => { const a = []; for (let i = 0; i < 3; i++) a.unshift(i); return a })()',
  'typeof (async function(){})',
  '(() => { const o = Object.create(null); o.x = 1; return o.x })()',
  '(() => { const s = new Set(); s.add(1).add(2); return [...s].length })()',
];
for (const expr of langMore) E.push(['lang-more', expr]);

// —— builtin 域：定制模板 ——————————————————————————————————

const builtinCases = [];

// path
const pathCases = [
  'require("path").join("a", "b", "c")',
  'require("path").join("/a", "../b")',
  'require("path").resolve("x") === require("path").resolve("x")',
  'require("path").basename("/a/b.txt")',
  'require("path").basename("/a/b.txt", ".txt")',
  'require("path").dirname("/a/b/c")',
  'require("path").extname("file.tar.gz")',
  'require("path").extname("noext")',
  'require("path").isAbsolute("/x/y")',
  'require("path").parse("/a/b/c.txt").ext',
  'require("path").normalize("a//b\\\\c/../d")',
  'require("path").relative("/a/b", "/a/c") === require("path").relative("/a/b", "/a/c")',
  'require("path").sep.length === 1',
];
for (const expr of pathCases) builtinCases.push(['builtin-path', expr]);

// util / assert
const utilCases = [
  'require("util").format("%s-%d", "a", 1)',
  'require("util").format("%j", {k:1})',
  'require("util").types.isPromise(Promise.resolve(1))',
  'require("util").inspect({a:1}) === require("util").inspect({a:1})',
  '(() => { const u = require("util"); const fn = u.promisify((cb) => cb(null, "ok")); return fn() instanceof Promise })()',
  '(() => { const a = require("assert"); a.equal(1, 1); return "eq-ok" })()',
  '(() => { try { require("assert").equal(1, 2) } catch (e) { return e.name } })()',
  '(() => { require("assert").deepEqual({a:[1]}, {a:[1]}); return "deep-ok" })()',
  '(() => { try { require("assert").ok(0) } catch (e) { return e.name } })()',
  'require("util").deprecate(() => 1)()',
];
for (const expr of utilCases) builtinCases.push(['builtin-util', expr]);

// os
const osCases = [
  'require("os").EOL.length',
  'require("os").arch() === require("os").arch()',
  'require("os").platform() === require("os").platform()',
  'require("os").type() === require("os").type()',
  'typeof require("os").hostname()',
  'require("os").cpus().length >= 1',
  'require("os").tmpdir().length > 0',
  'require("os").homedir().length > 0',
];
for (const expr of osCases) builtinCases.push(['builtin-os', expr]);

// querystring / url
const urlCases = [
  'require("querystring").parse("a=1&b=x%20y")',
  'require("querystring").stringify({a: "1", b: "x y"})',
  'new URL("https://x.io:8080/a/b?q=1#f").port',
  'new URL("https://x.io/a?b=1&c=2").searchParams.get("c")',
  '(() => { const u = new URL("https://x.io/a"); u.searchParams.set("k", "v v"); return u.search })()',
  'new URL("/rel", "https://x.io/base/").href',
  'require("url").format(new URL("https://x.io/a"))',
  'new URL("https://user:pw@x.io/").username',
  'decodeURIComponent("%E4%B8%AD")',
  'encodeURIComponent("中")',
];
for (const expr of urlCases) builtinCases.push(['builtin-url', expr]);

// buffer
const bufferCases = [
  'require("buffer").Buffer.from("abc").toString("hex")',
  'require("buffer").Buffer.from("abc").toString("base64")',
  'require("buffer").Buffer.from([104,105]).toString()',
  'require("buffer").Buffer.alloc(3, 1).length',
  'require("buffer").Buffer.alloc(3).fill(0)[0]',
  '(() => { const b = require("buffer").Buffer.from("hello"); b.write("X", 0); return b.toString() })()',
  'require("buffer").Buffer.from("hello").slice(1).toString()',
  'require("buffer").Buffer.from("hello").indexOf("l")',
  'require("buffer").Buffer.from("a=b").toJSON().type',
  'require("buffer").Buffer.from("中", "utf8").length',
  'require("buffer").Buffer.concat([Buffer.from("a"), Buffer.from("b")]).toString()',
  'require("buffer").Buffer.from("abc").byteLength',
  'require("buffer").Buffer.from("abc").equals(Buffer.from("abc"))',
  'require("buffer").Buffer.compare(Buffer.from("a"), Buffer.from("b"))',
  'require("buffer").Buffer.from("abc").readUInt8(0)',
  'require("buffer").Buffer.isBuffer(Buffer.from(""))',
];
for (const expr of bufferCases) builtinCases.push(['builtin-buffer', expr]);

// fs（cwd 内安全文件）
const fsCases = [
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t1.txt", "你好"); return fs.readFileSync("gc_fs_t1.txt", "utf8") })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t2.txt", "ab"); return fs.readFileSync("gc_fs_t2.txt").length })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t3.txt", "x"); fs.unlinkSync("gc_fs_t3.txt"); return fs.existsSync("gc_fs_t3.txt") })()',
  '(() => { const fs = require("fs"); fs.mkdirSync("gc_fs_d1"); fs.mkdirSync("gc_fs_d1/sub"); fs.writeFileSync("gc_fs_d1/sub/f.txt", "1"); return fs.readdirSync("gc_fs_d1/sub").join() })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t4.txt", "data"); return fs.statSync("gc_fs_t4.txt").isFile() })()',
  '(() => { const fs = require("fs"); return fs.statSync(".").isDirectory() })()',
  '(() => { const fs = require("fs"); try { fs.readFileSync("no_such_file_gc.txt") } catch (e) { return e.code } })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t5.txt", "5"); const fd = fs.openSync("gc_fs_t5.txt", "r"); const buf = Buffer.alloc(1); fs.readSync(fd, buf, 0, 1, 0); fs.closeSync(fd); return buf.toString() })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t6.txt", "a,c\\nb,d"); return fs.readFileSync("gc_fs_t6.txt", "utf8").split("\\n").length })()',
  '(() => { const fs = require("fs/promises"); return fs.readFile("package.json").then(b => b.length > 0) })()',
  '(() => { const fs = require("fs"); fs.appendFileSync("gc_fs_t7.txt", "a"); fs.appendFileSync("gc_fs_t7.txt", "b"); return fs.readFileSync("gc_fs_t7.txt", "utf8") })()',
  '(() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t8.json", JSON.stringify({ok:1})); return JSON.parse(fs.readFileSync("gc_fs_t8.json", "utf8")).ok })()',
];
for (const expr of fsCases) builtinCases.push(['builtin-fs', expr]);

// timers/process（确定性时序）
const procCases = [
  '(() => { return new Promise(r => setTimeout(() => r("t-done"), 5)) })()',
  '(() => { return new Promise(r => { const id = setTimeout(() => r("no"), 50); clearTimeout(id); setTimeout(() => r("cleared-ok"), 10) }) })()',
  '(() => { let n = 0; const id = setInterval(() => { n++; if (n === 2) { clearInterval(id) } }, 5); return new Promise(r => setTimeout(() => r(n), 40)) })()',
  '(() => { return new Promise(r => setImmediate(() => r("imm"))) })()',
  '(() => { const order = []; setImmediate(() => order.push("i")); Promise.resolve().then(() => order.push("p")); setTimeout(() => r2(order), 20); function r2(o) { r(o.join(",")) } var r; return new Promise(res => { r = res; }) })()',
  '(() => { const arr = [process.argv.length, typeof process.env, typeof process.exit]; return arr.join(",") })()',
  '(() => { return process.cwd().length > 0 })()',
  '(() => { return typeof process.stdout.write === "function" })()',
  '(() => { process.nextTick(() => {}); return "nt-ok" })()',
  '(() => { return [process.platform === require("os").platform(), process.version.startsWith("v")].join(",") })()',
];
for (const expr of procCases) builtinCases.push(['builtin-timers', expr]);

// events
const eventsCases = [
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let v = 0; e.on("x", (a) => { v = a }); e.emit("x", 7); return v })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let n = 0; e.once("x", () => n++); e.emit("x"); e.emit("x"); return n })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); e.on("x", () => {}); return e.listenerCount("x") })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); e.on("x", () => {}); e.removeAllListeners("x"); return e.listenerCount("x") })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); return e.emit("nothing") })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); const names = []; e.on("a", f); e.on("b", f); function f() {} return e.eventNames().length })()',
  '(() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let order = []; e.on("e", (v) => order.push(v)); return new Promise(r => { e.emit("e", 1); r(order.join()) }) })()',
];
for (const expr of eventsCases) builtinCases.push(['builtin-events', expr]);

// stream（确定性小数据）
const streamCases = [
  '(() => { const { Readable } = require("stream"); const r = Readable.from(["a", "b"]); const chunks = []; r.on("data", c => chunks.push(c.toString())); return new Promise(res => r.on("end", () => res(chunks.join("")))) })()',
  '(() => { const { Readable } = require("stream"); return Readable.from([1, 2, 3]).readableLength >= 0 })()',
  '(() => { const { Writable } = require("stream"); const w = new Writable({ write(chunk, enc, cb) { cb() } }); w.write("x"); w.end(); return new Promise(r => w.on("finish", () => r("w-done"))) })()',
  '(() => { const { pipeline } = require("stream/promises"); const { Readable } = require("stream"); const out = []; return pipeline(Readable.from(["p", "q"]), async function* (src) { for await (const c of src) yield c.toString().toUpperCase() }, async function* (src) { for await (const c of src) { out.push(c) } }).then(() => out.join("")) })()',
  '(() => { const { Readable } = require("stream"); const r = new Readable({ read() {} }); r.push("x"); r.push(null); return new Promise(res => { let s = ""; r.on("data", c => s += c); r.on("end", () => res(s)) }) })()',
  '(() => { const { Transform } = require("stream"); const t = new Transform({ transform(chunk, enc, cb) { cb(null, chunk.toString().trim()) } }); t.end("  z  "); return new Promise(r => { let s = ""; t.on("data", c => s += c); t.on("end", () => r(s)) }) })()',
];
for (const expr of streamCases) builtinCases.push(['builtin-stream', expr]);

// console / json 对称
const miscCases = [
  '(() => { console.log("log-line"); return "logged" })()',
  '(() => { console.error("err-line"); return "erred" })()',
  '(() => { console.info("info-line"); return "infoed" })()',
  '(() => { console.warn("warn-line"); return "warned" })()',
  '(() => { console.log("num", 1, "str", "s", "bool", false); return "multi" })()',
  '(() => { console.log({a: [1, 2]}); return "obj" })()',
];
for (const expr of miscCases) builtinCases.push(['misc-console', expr]);

// —— 系统化矩阵：方法 × 参数组合（诚实扩容，每条都是真实行为探针） ————
const MATRIX = [];

// String.prototype × 常见接收者
const strReceivers = ['"Hello World"', '""', '"a"', '"abc123"', '"  mixEd Case  "'];
const strMethods = [
  ['toUpperCase', ''], ['toLowerCase', ''], ['trim', ''], ['trimStart', ''], ['trimEnd', ''],
  ['length', null], ['charAt(0)', null], ['charAt(2)', null], ['charCodeAt(0)', null],
  ['indexOf("o")', null], ['lastIndexOf("o")', null], ['includes("o")', null],
  ['startsWith("H")', null], ['endsWith("d")', null], ['slice(0, 5)', null],
  ['substring(6)', null], ['repeat(2)', null], ['split("")', null],
  ['replace("l", "L")', null], ['replaceAll("l", "L")', null],
  ['padStart(15, ".")', null], ['padEnd(15, "-")', null],
];
for (const r of strReceivers) {
  for (const [m] of strMethods) MATRIX.push(['str-matrix', `(${r}).${m}`]);
}

// Array.prototype × 接收者
const arrReceivers = ['[1, 2, 3]', '[]', '[5, 1, 4]', '["b", "a"]', '[1, [2, 3], 4]'];
const arrMethods = [
  ['length', null], ['join()', null], ['join("-")', null], ['reverse()', null],
  ['slice(1)', null], ['slice(-1)', null], ['indexOf(2)', null], ['includes(1)', null],
  ['concat([9])', null], ['flat()', null], ['map(x => x)', null],
  ['filter(x => true)', null], ['every(x => true)', null], ['some(x => false)', null],
  ['reduce((a, b) => a, 0)', null], ['find(x => false)', null],
  ['entries().next().value', null], ['keys().next().value', null],
  ['toReversed()', null], ['at(0)', null],
];
for (const r of arrReceivers) {
  for (const [m] of arrMethods) MATRIX.push(['arr-matrix', `(${r}).${m}`]);
}

// Math 单参函数 × 数值
const mathFns = ['abs', 'ceil', 'floor', 'round', 'trunc', 'sign', 'sqrt', 'cbrt', 'exp', 'log1p'];
const mathArgs = ['0', '1', '-1', '0.5', '-2.5', '100', '1e-3'];
for (const f of mathFns) {
  for (const a of mathArgs) MATRIX.push(['math-matrix', `Math.${f}(${a})`]);
}

// Number.prototype × 接收者
const numReceivers = ['(123.456)', '(0)', '(-42.7)', '(255)', '(0.5)'];
const numMethods = ['toFixed(0)', 'toFixed(2)', 'toString()', 'toString(16)', 'toPrecision(3)', 'valueOf()'];
for (const r of numReceivers) {
  for (const m of numMethods) MATRIX.push(['num-matrix', `${r}.${m}`]);
}

// Object/JSON × 对象
const objSamples = ['{a: 1}', '{x: "s", y: [1]}', '{n: null}', '{d: new Date(0)}'];
for (const o of objSamples) {
  MATRIX.push(['obj-matrix', `Object.keys(${o})`]);
  MATRIX.push(['obj-matrix', `Object.values(${o})`]);
  MATRIX.push(['obj-matrix', `JSON.stringify(${o})`]);
  MATRIX.push(['obj-matrix', `JSON.parse(JSON.stringify(${o}))`]);
  MATRIX.push(['obj-matrix', `Object.entries(${o}).length`]);
}

// path 方法 × 输入
const pathInputs = ['"/a/b/c.txt"', '"a/b"', '"file.md"', '"/"', '"./x/y"'];
const pathFns = ['basename', 'dirname', 'extname'];
for (const f of pathFns) {
  for (const i of pathInputs) MATRIX.push(['path-matrix', `require("path").${f}(${i})`]);
}

// Buffer × 编码
const bufInputs = ['"abc"', '""', '"hello world"', '"é"'];
const bufEncs = ['"utf8"', '"hex"', '"base64"'];
for (const b of bufInputs) {
  for (const e of bufEncs) {
    MATRIX.push(['buf-matrix', `require("buffer").Buffer.from(${b}).toString(${e})`]);
  }
  MATRIX.push(['buf-matrix', `require("buffer").Buffer.from(${b}).length`]);
}


// —— 第二批矩阵 ————————————————————————————————————————
// Map/Set 方法矩阵
const setRecv = ['new Set([1,2,3])', 'new Set("ab")'];
const setOps = ['size', 'has(1)', 'has(9)'];
for (const r of setRecv) for (const o of setOps) MATRIX.push(['set-matrix', `(${r}).${o}`]);
for (const r of setRecv) {
  MATRIX.push(['set-matrix', `[...(${r}).values()]`]);
  MATRIX.push(['set-matrix', `[...(${r}).keys()]`]);
}
const mapRecv = ['new Map([["a",1],["b",2]])'];
const mapOps = ['size', 'get("a")', 'get("z")', 'has("b")'];
for (const r of mapRecv) for (const o of mapOps) MATRIX.push(['map-matrix', `(${r}).${o}`]);
for (const r of mapRecv) {
  MATRIX.push(['map-matrix', `[...(${r}).keys()].length`]);
  MATRIX.push(['map-matrix', `[...(${r}).values()].length`]);
}

// URI 编解码矩阵
const uriInputs = ['"a b"', '"a=b&c=d"', '"中"', '"%E4%B8%AD"', '"abc123!@#"'];
for (const i of uriInputs) {
  MATRIX.push(['uri-matrix', `encodeURIComponent(${i})`]);
  MATRIX.push(['uri-matrix', `encodeURI(${i})`]);
}
for (const i of ['"%E4%B8%AD"', '"a%20b"', '"abc"']) {
  MATRIX.push(['uri-matrix', `decodeURIComponent(${i})`]);
  MATRIX.push(['uri-matrix', `decodeURI(${i})`]);
}

// 全局对象面
const globals = ['Object', 'Array', 'JSON', 'Math', 'Promise', 'Map', 'Set', 'RegExp', 'Date', 'Number', 'String', 'Boolean', 'Error', 'TypeError', 'Symbol', 'Proxy', 'Reflect', 'Intl'];
for (const g of globals) MATRIX.push(['global-matrix', `typeof ${g}`]);

// Promise 静态/实例面（同步可观测）
const promiseProbes = [
  'typeof Promise.resolve', 'typeof Promise.reject', 'typeof Promise.all',
  'typeof Promise.race', 'typeof Promise.allSettled', 'typeof Promise.any',
  'typeof Promise.prototype.then', 'typeof Promise.prototype.catch',
  'typeof Promise.prototype.finally',
  'Promise.resolve(1) instanceof Promise',
];
for (const pr of promiseProbes) MATRIX.push(['promise-matrix', pr]);

// eval / typeof / instanceof 矩阵
const evalProbes = [
  'eval("1 + 2")', 'eval("[1, 2].length")', 'eval("var ev = 9; ev")',
  'typeof eval', 'typeof (() => {}) === "function"',
  '[] instanceof Array', '({}) instanceof Object', '(new Map()) instanceof Map',
  '(1) instanceof Object', '("s") instanceof String',
];
for (const ev of evalProbes) MATRIX.push(['eval-matrix', ev]);

// 正则矩阵
const regexProbes = [
  '/\d+/.exec("abc123def")?.[0]', '/^a/.test("abc")', '/c$/.test("abc")',
  '"2024-06-01".match(/\d{4}/)[0]', '"aB".replace(/b/, "X")',
  'new RegExp("ab?", "i").test("AB")', '"x,y".split(",")',
  'Array.from("a1b2".matchAll(/\d/g)).length',
  '/(?<year>\d{4})/.exec("2024")?.groups.year',
];
for (const r of regexProbes) MATRIX.push(['regex-matrix', r]);

// JSON 边界
const jsonProbes = [
  'JSON.stringify(true)', 'JSON.stringify(null)', 'JSON.stringify(undefined)',
  'JSON.stringify(0.5)', 'JSON.stringify(-0)', 'JSON.stringify([[[[1]]]])',
  'JSON.stringify({a:{b:{c:1}}})', 'JSON.parse("[]")', 'JSON.parse("[1]")',
  'JSON.parse("true")', 'JSON.stringify("plain")',
  'JSON.parse(JSON.stringify({a:1}), (k, v) => (k === "a" ? v * 2 : v))',
  'JSON.stringify({a: [1]}) === String(JSON.stringify({a: [1]}))',
];
for (const j of jsonProbes) MATRIX.push(['json-matrix', j]);

// 日期静态与固定时间戳
const dateProbes = [
  'new Date(0).getTime()', 'new Date(0).toISOString()',
  'new Date(86400000).toISOString()', 'new Date(0).toJSON()',
  'Date.parse("1970-01-01T00:00:01Z")', 'Date.UTC(1970, 0, 1)',
  'new Date(1234567890123).getTime() === 1234567890123',
  'typeof Date.now()', 'Date.now() >= 0',
  'new Date("invalid").getTime()',
];
for (const d of dateProbes) MATRIX.push(['date-matrix', d]);

// Symbol/迭代协议
const symProbes = [
  'typeof Symbol()', 'typeof Symbol("tag")', 'Symbol("a").toString()',
  'Symbol.for("k") === Symbol.for("k")', 'Symbol("a") === Symbol("a")',
  'Symbol.keyFor(Symbol.for("g"))',
  '(() => { const o = {}; o[Symbol.iterator] = function*(){ yield 1 }; return [...o] })()',
  'typeof [][Symbol.iterator]',
];
for (const s of symProbes) MATRIX.push(['symbol-matrix', s]);

// 类型转换矩阵
const coerceProbes = [
  'String({})', 'String([1,2])', 'String(true)', '[].toString()',
  'Boolean("0")', 'Boolean([])', 'Number("12px")', 'Number(true)',
  'parseInt("11", 2)', 'parseInt("ff", 16)', 'parseInt("z", 36)',
  '(true + 1)', '(false * 3)', '(null == undefined)', '(null === undefined)',
  '(NaN !== NaN)', '(!undefined)', '(!!"false")',
  'Array(3).join()', 'typeof String(undefined)',
];
for (const c of coerceProbes) MATRIX.push(['coerce-matrix', c]);

// —— 第三批矩阵（补量） ——————————————————————————————————————
const math2 = [['2', '3'], ['0', '-1'], ['1.5', '2']];
for (const f of ['max', 'pow', 'min']) {
  for (const [a, b] of math2) MATRIX.push(['math-matrix', `Math.${f}(${a}, ${b})`]);
}
const moreStr = ['"abcdef"'.replace(/"/g, '')];
const strExtras = [
  ['"abcdef"'].flatMap(r => [['slice(2, 4)'], ['indexOf("c")'], ['charAt(3)'], ['includes("de")']]),
];
for (const r of ['"abcdef"']) {
  for (const [m] of strExtras.flat()) MATRIX.push(['str-matrix', `(${r}).${m}`]);
}
const moreArrs = ['[9, 8, 7, 6]', '["x", "y"]'];
const moreArrOps = ['map(x => x * 2)', 'filter(x => x > 7)', 'slice(0, 2)', 'join("+")', 'at(-2)', 'reverse()'];
for (const r of moreArrs) for (const m of moreArrOps) MATRIX.push(['arr-matrix', `(${r}).${m}`]);
const moreCoerce = [
  'String(123)', 'String(-4.5)', '(1 && 2)', '(0 || "x")', '(null ?? 0)',
  '(+true)', '(-false)', '(~0)', '(~~3.7)', '(5 | 0)', '(5.9 | 0)',
  'parseInt("", 10)', 'parseFloat("2.5e3")', '(new Number(5) == 5)',
  'typeof new String("s")', 'Array.isArray([])', 'Array.isArray("[]")',
];
for (const c of moreCoerce) MATRIX.push(['coerce-matrix', c]);
const moreJson = [
  'JSON.stringify([])', 'JSON.stringify({})', 'JSON.stringify("")',
  'JSON.parse("{}")', 'JSON.parse("[1, 2]")', 'JSON.stringify(1e-6)',
  'JSON.stringify(Symbol("x"))', 'JSON.stringify([undefined])',
];
for (const j of moreJson) MATRIX.push(['json-matrix', j]);

// —— 第四批（冲 1000） ——————————————————————————————————————
const corePairs = [
  ['7 % 3', '-7 % 3', '7.5 % 2', '2 ** 0', '0 ** 0', '(-2) ** 3'],
  ['(16).toString()', '1 .constructor.name'],
  ['[1, 2].toString()', '[[], []].toString()', '[1, [2, [3]]].toString()'],
  ['"a" < "b"', '"A" < "a"', '"2" > "10"', '(1 < "2")'],
  ['(typeof (() => { }) === "function")', '(void 0 === undefined)'],
  ['(5, 6, 7)', '((a => a * 2)(4))'],
  ['(function* g() { yield* [1, 2] })', 'typeof (function* () { })'],
  ['(new Array(3).length)', '(Array.of(7).length)', '(Array.of().length)'],
  ['([10].pop())', '([10, 20].shift())', '([1].unshift(9))'],
  ['("x" + null)', '("x" + undefined)', '("x" + true)'],
];
for (const row of corePairs) {
  for (const expr of row) MATRIX.push(['core-pairs', expr]);
}

// —— 第五批（补到 1000+） —————————————————————————————————————
const finalProbes = [
  'JSON.stringify({ get x() { return 1 } })',
  'Object.keys([1, 2]).length',
  'Object.getOwnPropertySymbols({}).length',
  '(function (...a) { return a.length })(1, 2, 3)',
  '((new Date(0)).valueOf())',
  'isNaN("x")', 'isNaN("")',
  '(1..toFixed)', '(255).toString(2).length',
  '(() => { const { floor } = Math; return floor(9.9) })()',
  '(() => { try { (void 0).x } catch (e) { return e instanceof TypeError } })()',
  '(() => { const a = [1]; a.length = 3; return a[2] })()',
  'delete undefined',
  '(void delete 1)',
  '(-"3")', '(-"x")', '(+"")', '(+true)',
  '("5" | 0)', '("5" ^ 0)', '("3" & 1)',
  '((0.1 + 0.2) === 0.3)', '(Math.abs(0.1 + 0.2 - 0.3) < 1e-10)',
  '(2 ** 53 === 2 ** 53 + 0)',
];
for (const pr of finalProbes) MATRIX.push(['final-matrix', pr]);

for (const [d, e] of MATRIX) E.push([d, e]);

// —— 组装 & 产出 ————————————————————————————————————————————

const CASES = [];
for (const [domain, expr] of E) {
  CASES.push([domain, `try { console.log(JSON.stringify(${expr})) } catch (e) { console.log("ERR", e.name) }`]);
}
for (const [domain, expr] of builtinCases) {
  CASES.push([domain, `try { (async () => console.log(JSON.stringify(await (${expr}))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }`]);
}

// 清空旧的生成用例，写新一批
for (const f of readdirSync(target)) {
  if (/^gen-.+\.cjs$/.test(f)) rmSync(join(target, f));
}
const counters = {};
let total = 0;
for (const [domain, body] of CASES) {
  counters[domain] = (counters[domain] ?? 0) + 1;
  const name = `gen-${domain}-${String(counters[domain]).padStart(4, '0')}.cjs`;
  const src = `// conformance 生成用例（gen.mjs 产出，勿手改）：${domain}\n${body}\n`;
  writeFileSync(join(target, name), src);
  total++;
}
console.log(`generated ${total} cases across ${Object.keys(counters).length} domains`);
