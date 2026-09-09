//! 迭代器协议差分测试（对照 Node.js 22 LTS）：字符串 / 数组 / Map / Set。
//!
//! 覆盖：
//! - 字符串 `for...of` 逐码点（含代理对）、`[...str]` 展开
//! - 数组 `for...of` / keys / entries / 迭代器复用
//! - Map 构造（iterable 形态）+ for...of / keys / values / entries / forEach +
//!   原位更新保插入序 + `[Symbol.iterator]()` 手工迭代
//! - Set 构造去重 + for...of / keys / entries（`[v, v]` 形态）/ forEach
//! - Map/Set 展开进数组字面量

mod common;

/// 通用探针执行：写 JS → 与 Node 22 对拍 → 返回 stdout。
fn run_probe(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("iter_protocol_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_node(&work, "probe.js")
}

/// 字符串逐码点迭代：BMP 与代理对均按 Unicode 码点产出。
#[test]
fn string_for_of_iterates_by_code_point() {
    let out = run_probe(
        "str_codepoint",
        r#"
let s = '';
for (const ch of 'a\u{1F600}b') s += ch + '|';
console.log(s);
console.log([...'xy'].join(','));
const si = 'hi'[Symbol.iterator]();
let r = si.next();
console.log(r.value, r.done);
r = si.next();
console.log(r.value, r.done);
r = si.next();
console.log(r.value === undefined ? 'undefined' : r.value, r.done);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "a|😀|b|");
    assert_eq!(lines[1], "x,y");
    assert_eq!(lines[2], "h false");
    assert_eq!(lines[3], "i false");
    assert_eq!(lines[4], "undefined true");
}

/// 数组 for...of 与 keys/entries 迭代器。
#[test]
fn array_iterators_matches_node() {
    let out = run_probe(
        "arr_iter",
        r#"
let sum = 0;
for (const x of [1, 2, 3]) sum += x;
console.log(sum);
let ks = [];
for (const k of [10, 20].keys()) ks.push(k);
console.log(JSON.stringify(ks));
let es = [];
for (const e of ['p', 'q'].entries()) es.push(e[0] + ':' + e[1]);
console.log(JSON.stringify(es));
const it = [5, 6].values();
let c = 0;
for (const x of it) c += x; // 迭代器对象自身可再迭代
console.log(c);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "6");
    assert_eq!(lines[1], "[0,1]");
    assert_eq!(lines[2], r#"["0:p","1:q"]"#);
    assert_eq!(lines[3], "11");
}

/// Map：构造（iterable）+ 全迭代形态 + 原位更新保插入序。
#[test]
fn map_iterators_matches_node() {
    let out = run_probe(
        "map_iter",
        r#"
const m = new Map([['a', 1], ['b', 2]]);
console.log(m.size);
let pairs = [];
for (const kv of m) pairs.push(kv[0] + '=' + kv[1]);
console.log(JSON.stringify(pairs));
let ks = [];
for (const k of m.keys()) ks.push(k);
console.log(JSON.stringify(ks));
let vs = [];
for (const v of m.values()) vs.push(v);
console.log(JSON.stringify(vs));
let acc = '';
m.forEach((v, k) => { acc += k + v; });
console.log(acc);
const m2 = new Map();
m2.set('x', 1);
m2.set('y', 2);
m2.set('x', 9); // 原位更新 → 插入序保持 x,y
let o = [];
for (const kv of m2) o.push(kv.join('='));
console.log(JSON.stringify(o));
const first = m[Symbol.iterator]().next();
console.log(JSON.stringify([first.done, first.value]));
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "2");
    assert_eq!(lines[1], r#"["a=1","b=2"]"#);
    assert_eq!(lines[2], r#"["a","b"]"#);
    assert_eq!(lines[3], "[1,2]");
    assert_eq!(lines[4], "a1b2");
    assert_eq!(lines[5], r#"["x=9","y=2"]"#);
    assert_eq!(lines[6], r#"[false,["a",1]]"#);
}

/// Set：构造去重 + 迭代形态（entries 产出 [v, v]）+ forEach 双参。
#[test]
fn set_iterators_matches_node() {
    let out = run_probe(
        "set_iter",
        r#"
const st = new Set([1, 2, 2, 3]);
console.log(st.size);
let els = [];
for (const v of st) els.push(v);
console.log(JSON.stringify(els));
let ks = [];
for (const k of st.keys()) ks.push(k);
console.log(JSON.stringify(ks));
let es = [];
for (const e of st.entries()) es.push(JSON.stringify(e));
console.log(JSON.stringify(es));
let acc = 0;
st.forEach((v, v2) => { acc += v * 10 + v2; });
console.log(acc);
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "[1,2,3]");
    assert_eq!(lines[2], "[1,2,3]");
    assert_eq!(lines[3], r#"["[1,1]","[2,2]","[3,3]"]"#);
    assert_eq!(lines[4], "66"); // 11 + 22 + 33
}

/// 展开语法：字符串 / Map / Set 进数组字面量。
#[test]
fn spread_syntax_matches_node() {
    let out = run_probe(
        "spread",
        r#"
console.log(JSON.stringify([...'xy']));
console.log(JSON.stringify([...new Map([['k', 'v']])]));
console.log(JSON.stringify([...new Set(['a', 'b'])]));
console.log(JSON.stringify([1, ...'ab', 2]));
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], r#"["x","y"]"#);
    assert_eq!(lines[1], r#"[["k","v"]]"#);
    assert_eq!(lines[2], r#"["a","b"]"#);
    assert_eq!(lines[3], r#"[1,"a","b",2]"#);
}
