//! VM 核心语义缺口回归测试（对齐 Node.js 22 LTS 标准 逐字对拍）：
//! - Promise 拒绝语义（`.catch`、`Promise.reject`、await 拒绝传播、构造器执行器）
//! - 基础内建：`String()` / `Array.isArray` / `Object.keys` / `JSON.stringify`
//! - 字符串原型方法（trim/indexOf/slice 等）与 `fn.name`
//! - 数组 `for...of` 迭代
//!
//! 每个用例：Go 前端整图编译 → aluvm 执行 → Node.js 22 LTS 标准 源码执行，输出逐字一致。

mod common;

/// 通用探针执行：写入 js → 三方对拍 → 返回 stdout。
fn run_probe(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("core_semantics_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_go(&work, "probe.js")
}

/// Promise 拒绝：已拒绝 promise 的 `.catch` 立即调度；`Promise.reject` 静态构造。
#[test]
fn promise_rejection_catch_matches_go() {
    let out = run_probe(
        "promise_catch",
        concat!(
            "new Promise(function(_, rej) { rej(new Error(\"boom\")); })",
            ".catch(function(e) { console.log(\"caught:\", e.message); });\n",
            "Promise.reject(new Error(\"pre\")).catch(function(e) { console.log(\"pre-caught:\", e.message); });\n",
        ),
    );
    assert_eq!(out, "caught: boom\npre-caught: pre");
}

/// await 已拒绝的 promise：拒绝原因在帧内 try/catch 抛出（非 resolve(undefined) 续跑）。
#[test]
fn await_rejection_throws_in_frame_matches_go() {
    let out = run_probe(
        "await_reject",
        concat!(
            "async function main() {\n",
            "    try {\n",
            "        await Promise.reject(new Error(\"await-err\"));\n",
            "        console.log(\"NOT REACHED\");\n",
            "    } catch (e) {\n",
            "        console.log(\"await caught:\", e.message);\n",
            "    }\n",
            "}\n",
            "main();\n",
        ),
    );
    assert_eq!(out, "await caught: await-err");
}

/// 异步链式拒绝：无 catch 的 async 函数体内抛错 → 其 promise 拒绝 → 上层 catch 接住。
#[test]
fn async_throw_rejects_promise_chain_matches_go() {
    let out = run_probe(
        "async_chain",
        concat!(
            "async function inner() {\n",
            "    throw new Error(\"inner-boom\");\n",
            "}\n",
            "async function outer() {\n",
            "    try {\n",
            "        await inner();\n",
            "        console.log(\"NOT REACHED\");\n",
            "    } catch (e) {\n",
            "        console.log(\"chained:\", e.message);\n",
            "    }\n",
            "}\n",
            "outer();\n",
        ),
    );
    assert_eq!(out, "chained: inner-boom");
}

/// 构造器执行器同步抛错：promise 以该异常拒绝。
#[test]
fn promise_executor_sync_throw_rejects_matches_go() {
    let out = run_probe(
        "executor_throw",
        concat!(
            "new Promise(function() {\n",
            "    throw new Error(\"exec-boom\");\n",
            "}).catch(function(e) { console.log(\"exec caught:\", e.message); });\n",
        ),
    );
    assert_eq!(out, "exec caught: exec-boom");
}

/// 基础内建：String() / Array.isArray / Object.keys / JSON.stringify（含 Go 顶层
/// undefined → "null" 怪癖）。
#[test]
fn basic_globals_match_go() {
    let out = run_probe(
        "basic_globals",
        concat!(
            "console.log(\"String:\", String(42), String(null), String(true));\n",
            "console.log(\"isArray:\", Array.isArray([1]), Array.isArray(\"x\"), Array.isArray({}));\n",
            "var ks = Object.keys({ a: 1, b: 2, c: 3 });\n",
            "console.log(\"keys:\", ks.length, ks[0], ks[1], ks[2]);\n",
            "console.log(\"json1:\", JSON.stringify({ a: 1, b: \"x\" }));\n",
            "console.log(\"json2:\", JSON.stringify(undefined));\n",
            "console.log(\"json3:\", JSON.stringify(\"str\"));\n",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "String: 42 null true\n",
            "isArray: true false false\n",
            "keys: 3 a b c\n",
            "json1: {\"a\":1,\"b\":\"x\"}\n",
            "json2: undefined\n",
            "json3: \"str\"",
        )
    );
}

/// 字符串原型方法与 length。
#[test]
fn string_prototype_methods_match_go() {
    let out = run_probe(
        "string_methods",
        concat!(
            "var t = \"  hi  \";\n",
            "console.log(\"trim:\", \"[\" + t.trim() + \"]\");\n",
            "console.log(\"indexOf:\", \"hello\".indexOf(\"l\"));\n",
            "console.log(\"slice:\", \"hello\".slice(1, 3));\n",
            "console.log(\"len:\", \"hello\".length, \"up:\", \"hi\".toUpperCase());\n",
            "console.log(\"split:\", \"a,b,c\".split(\",\").length, \"includes:\", \"hello\".includes(\"ell\"));\n",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "trim: [hi]\n",
            "indexOf: 2\n",
            "slice: el\n",
            "len: 5 up: HI\n",
            "split: 3 includes: true",
        )
    );
}

/// 数组 for...of 迭代（var/const 绑定）。
#[test]
fn for_of_array_iteration_matches_go() {
    let out = run_probe(
        "for_of",
        concat!(
            "var arr = [10, 20, 30];\n",
            "var sum = 0;\n",
            "for (var x of arr) { sum = sum + x; }\n",
            "console.log(\"forof:\", sum);\n",
            "for (const s of [\"a\", \"b\"]) { console.log(\"item:\", s); }\n",
        ),
    );
    assert_eq!(out, "forof: 60\nitem: a\nitem: b");
}

/// 函数 `name` 元属性（Go 编译产物携带函数名）。
#[test]
fn function_name_property_matches_go() {
    let out = run_probe(
        "fn_name",
        concat!(
            "function named() {}\n",
            "console.log(named.name === \"named\" ? \"named\" : \"other\");\n",
        ),
    );
    assert_eq!(out, "named");
}

/// Promise.all：混合即期值与 promise；任一拒绝立即拒绝组合器。
#[test]
fn promise_all_matches_go() {
    let out = run_probe(
        "promise_all",
        concat!(
            "Promise.all([Promise.resolve(1), 2, Promise.resolve(3)]).then(function(vs) {
",
            "    console.log(\"all:\", vs[0], vs[1], vs[2]);
",
            "});
",
            "Promise.all([Promise.resolve(1), Promise.reject(new Error(\"all-err\"))]).catch(function(e) {
",
            "    console.log(\"all-catch:\", e.message);
",
            "});
",
        ),
    );
    assert_eq!(
        out,
        "all: 1 2 3
all-catch: all-err"
    );
}

/// Promise.race：首个定型者胜（兑现与拒绝两形态）。
#[test]
fn promise_race_matches_go() {
    let out = run_probe(
        "promise_race",
        concat!(
            "Promise.race([Promise.resolve(\"win\"), Promise.resolve(\"lose\")]).then(function(v) {
",
            "    console.log(\"race:\", v);
",
            "});
",
            "Promise.race([Promise.reject(new Error(\"race-err\")), Promise.resolve(\"x\")]).catch(function(e) {
",
            "    console.log(\"race-catch:\", e.message);
",
            "});
",
        ),
    );
    assert_eq!(
        out,
        "race: win
race-catch: race-err"
    );
}

/// Promise.allSettled：永不拒绝，status/value/reason 形态。
#[test]
fn promise_all_settled_matches_go() {
    let out = run_probe(
        "promise_all_settled",
        concat!(
            "Promise.allSettled([Promise.resolve(1), Promise.reject(new Error(\"e2\"))]).then(function(rs) {
",
            "    console.log(\"settled:\", rs.length, rs[0].status, rs[0].value, rs[1].status, rs[1].reason.message);
",
            "});
",
        ),
    );
    assert_eq!(out, "settled: 2 fulfilled 1 rejected e2");
}

/// promise.finally(cb)：cb 运行且兑现值透传。
#[test]
fn promise_finally_passes_value_matches_go() {
    let out = run_probe(
        "promise_finally",
        concat!(
            "Promise.resolve(\"fv\").finally(function() { console.log(\"finally ran\"); }).then(function(v) {
",
            "    console.log(\"after-finally:\", v);
",
            "});
",
        ),
    );
    assert_eq!(
        out,
        "finally ran
after-finally: fv"
    );
}

/// Symbol 基础：typeof、显示形态、唯一性、String()、for 注册表与 keyFor。
#[test]
fn symbol_basics_match_go() {
    let out = run_probe(
        "symbol_basics",
        concat!(
            "var s1 = Symbol(\"d\");
",
            "var s2 = Symbol(\"d\");
",
            "console.log(\"typeof:\", typeof s1);
",
            "console.log(\"log:\", s1);
",
            "console.log(\"eq:\", s1 === s2);
",
            "console.log(\"str:\", String(s1));
",
            "var a = Symbol.for(\"k\");
",
            "var b = Symbol.for(\"k\");
",
            "console.log(\"for-eq:\", a === b, \"keyFor:\", Symbol.keyFor(a));
",
            "console.log(\"keyFor-miss:\", Symbol.keyFor(s1));
",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "typeof: symbol
",
            "log: Symbol(d)
",
            "eq: false
",
            "str: Symbol(d)
",
            "for-eq: true keyFor: k
",
            "keyFor-miss: undefined",
        )
    );
}

/// 符号属性键：读写、Object.keys 遮蔽、getOwnPropertySymbols、JSON 跳过。
#[test]
fn symbol_property_keys_match_go() {
    let out = run_probe(
        "symbol_keys",
        concat!(
            "var sym = Symbol(\"id\");
",
            "var o = {};
",
            "o[sym] = 42;
",
            "console.log(\"get:\", o[sym]);
",
            "console.log(\"keys:\", Object.keys(o).length);
",
            "console.log(\"syms:\", Object.getOwnPropertySymbols(o).length);
",
            "var o2 = { regular: 1 };
",
            "o2[Symbol(\"t\")] = 2;
",
            "console.log(\"keys2:\", Object.keys(o2).length, Object.getOwnPropertySymbols(o2).length);
",
            "console.log(\"json-sym-key:\", JSON.stringify(o2));
",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "get: 42
",
            "keys: 0
",
            "syms: 1
",
            "keys2: 1 1
",
            "json-sym-key: {\"regular\":1}",
        )
    );
}

/// 知名符号：typeof、相互唯一、for 注册表一致；description 取值对齐 **Node 22**。
///
/// 注：本用例原名 `match_go`，期望值录自 Go 基线（其 `Symbol.prototype.description`
/// 未实现 → 期望 `n/a`）。Node 22 实测 `typeof Symbol("d").description === "string"`
/// → 应为 `has`；按项目「以 Node 22 LTS 为唯一权威 oracle」的原则，期望值随实现
/// 对齐 Node 后更新（并已在属性读面补齐 `description` 的真实取值合成：
/// `Symbol("d").description === "d"`、`Symbol().description === undefined`）。
#[test]
fn symbol_well_known_match_go() {
    let out = run_probe(
        "symbol_well_known",
        concat!(
            "console.log(\"wk-typeof:\", typeof Symbol.iterator);
",
            "console.log(\"wk-neq:\", Symbol.iterator === Symbol.asyncIterator);
",
            "console.log(\"wk-same:\", Symbol.for(\"x\") === Symbol.for(\"x\"));
",
            "console.log(\"desc:\", typeof Symbol(\"d\").description === \"undefined\" ? \"n/a\" : \"has\");
",
            "console.log(\"sym-empty:\", typeof Symbol());
",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "wk-typeof: symbol
",
            "wk-neq: false
",
            "wk-same: true
",
            "desc: has
",
            "sym-empty: symbol",
        )
    );
}

/// GC 压力：40 万+ 分配触发多轮 major 回收，存活数据跨回收保持正确
/// （临时对象树、循环结构、闭包捕获、注册符号）。输出与 Go 逐字一致。
#[test]
fn gc_stress_heavy_allocation_matches_go() {
    let out = run_probe(
        "gc_stress",
        concat!(
            "let keep = [];
",
            "for (let i = 0; i < 50; i++) {
",
            "    let tmp = [];
",
            "    for (let j = 0; j < 2000; j++) {
",
            "        tmp.push({ a: j, b: \"s\" + j, c: [j, j + 1] });
",
            "    }
",
            "    keep.push(tmp[0].a + \"|\" + tmp[1999].b);
",
            "}
",
            "console.log(\"keep:\", keep.length, keep[0], keep[49]);
",
            "for (let i = 0; i < 30000; i++) {
",
            "    let x = { v: i };
",
            "    let y = { p: x };
",
            "    x.p = y;
",
            "}
",
            "console.log(\"cycles done\");
",
            "let fns = [];
",
            "for (let i = 0; i < 5000; i++) {
",
            "    let s = Symbol(\"s\" + i);
",
            "    let o = {};
",
            "    o[s] = i;
",
            "    fns.push(function() { return i; });
",
            "}
",
            "console.log(\"fns:\", fns.length, fns[0](), fns[4999]());
",
            "let reg1 = Symbol.for(\"gck\");
",
            "let reg2 = Symbol.for(\"gck\");
",
            "console.log(\"reg:\", reg1 === reg2);
",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "keep: 50 0|s1999 0|s1999
",
            "cycles done
",
            "fns: 5000 0 4999
",
            "reg: true",
        )
    );
}

/// 深递归：大栈线程下 fib(25)/fib(30) 与 Go 一致（原 1MB 主线程栈 fib(20) 溢出）。
#[test]
fn deep_recursion_fib_matches_go() {
    let out = run_probe(
        "deep_recursion",
        concat!(
            "function fib(n) {\n",
            "    if (n < 2) { return n; }\n",
            "    return fib(n - 1) + fib(n - 2);\n",
            "}\n",
            "console.log(\"fib25:\", fib(25));\n",
            "console.log(\"fib30:\", fib(30));\n",
        ),
    );
    assert_eq!(out, "fib25: 75025\nfib30: 832040");
}

/// Rust 全链路（alukac 编译 → aluvm 执行）：非 ASCII 字面量不再 mojibake，
/// 模板串插值/多字节字符正确（对照 Node.js 22 LTS 标准 同源码输出）。
#[test]
fn rust_frontend_cjk_literals_match_go() {
    let work = std::env::temp_dir().join(format!("rust_frontend_cjk_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let js = concat!(
        "console.log(\"中文测试\");\n",
        "console.log(\"日本語テスト\");\n",
        "console.log(\"中文\".length);\n",
        "console.log(\"混合abc中文字符\".slice(2, 4));\n",
        "let name = \"世界\";\n",
        "console.log(`你好, ${name}!`);\n",
    );
    std::fs::write(work.join("cjk.js"), js).unwrap();
    let rust_out = common::rust_pipeline_run(&work, "cjk.js");
    if let Some(node_out) = common::node_run(&work.join("cjk.js")) {
        assert_eq!(
            rust_out, node_out,
            "Rust 前端全链路输出必须与 Node.js 22 LTS 标准 一致"
        );
    }
    assert_eq!(rust_out, "中文测试\n日本語テスト\n2\nab\n你好, 世界!");
}

/// JSON.parse：嵌套结构、数字（负数/小数/指数）、字符串转义、错误消息
/// （标准 JSON 语法与 SyntaxError 错误）。
#[test]
fn json_parse_matches_node() {
    let out = run_probe(
        "json_parse",
        concat!(
            "var o = JSON.parse('{\"a\":1,\"b\":[true,false,null],\"c\":{\"d\":\"str\"}}');\n",
            "console.log(\"a:\", o.a, \"b0:\", o.b[0], \"b2:\", o.b[2], \"cd:\", o.c.d);\n",
            "console.log(\"neg:\", JSON.parse(\"-12.5e2\"));\n",
            "console.log(\"ws:\", JSON.parse(\"  [ 1 , 2 ]  \")[1]);\n",
            "console.log(\"esc:\", JSON.parse('\"a\\\\\"b\"'));\n",
            "try { JSON.parse(\"{bad}\"); } catch (e) { console.log(\"e1:\", e.name, e.message); }\n",
            "try { JSON.parse(''); } catch (e) { console.log(\"e2:\", e.message); }\n",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "a: 1 b0: true b2: null cd: str\n",
            "neg: -1250\n",
            "ws: 2\n",
            "esc: a\"b\n",
            "e1: SyntaxError invalid character 'b' looking for beginning of object key string\n",
            "e2: unexpected end of JSON input",
        )
    );
}

/// 自定义可迭代对象：Symbol.iterator 属性接入 for...of 协议。
#[test]
fn custom_iterable_for_of_matches_go() {
    let out = run_probe(
        "custom_iterable",
        concat!(
            "var bag = {};\n",
            "bag[Symbol.iterator] = function() {\n",
            "    let i = 0;\n",
            "    let self = {\n",
            "        next: function() {\n",
            "            i = i + 1;\n",
            "            if (i <= 3) { return { value: i * 10, done: false }; }\n",
            "            return { value: undefined, done: true };\n",
            "        }\n",
            "    };\n",
            "    return self;\n",
            "};\n",
            "let sum = 0;\n",
            "for (let x of bag) { sum = sum + x; }\n",
            "console.log(\"sum:\", sum);\n",
            "let items = [];\n",
            "for (let y of bag) { items.push(y); }\n",
            "console.log(\"items:\", items.join(\",\"));\n",
        ),
    );
    assert_eq!(out, "sum: 60\nitems: 10,20,30");
}

/// promise.then 链式语义：返回值采纳（含 promise 采纳）、回调抛错拒绝、
/// 拒绝透传、finally 透传、微任务时序与 Go 逐字一致。
#[test]
fn then_chaining_semantics_match_go() {
    let out = run_probe(
        "then_chaining",
        concat!(
            "Promise.resolve(1).then(function(v) { return v + 1; })\n",
            "  .then(function(v) { console.log(\"chain:\", v); });\n",
            "Promise.resolve(\"x\").then(function() { throw new Error(\"cb-throw\"); })\n",
            "  .catch(function(e) { console.log(\"caught:\", e.message); });\n",
            "Promise.reject(new Error(\"pass\")).then(function(v) { console.log(\"NOT REACHED\"); })\n",
            "  .catch(function(e) { console.log(\"pass-through:\", e.message); });\n",
            "Promise.resolve(\"fv\").finally(function() { console.log(\"fin-ran\"); })\n",
            "  .then(function(v) { console.log(\"fin-value:\", v); });\n",
            "Promise.resolve(2).then(function(v) { return Promise.resolve(v * 10); })\n",
            "  .then(function(v) { console.log(\"adopt:\", v); });\n",
        ),
    );
    assert_eq!(
        out,
        concat!(
            "fin-ran\n",
            "chain: 2\n",
            "caught: cb-throw\n",
            "pass-through: pass\n",
            "fin-value: fv\n",
            "adopt: 20",
        )
    );
}
