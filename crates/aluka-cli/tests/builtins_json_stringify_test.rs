//! JSON.stringify 差分测试（对照 Node.js 22 LTS）。
//!
//! 覆盖本次修复的规范语义：
//! - 对象键序 = 整数索引键升序前置 + 其余按**创建序**（shape 与字典模式均保序）
//! - 属性值为 undefined/函数 → 整键剔除；数组元素同值 → "null" 占位
//! - 顶层 undefined/函数 → 返回 undefined
//! - 删除后重加的键落在键序末尾（V8 慢化语义）
//! - 超字典阈值（>32 键）对象仍保插入序

mod common;

fn run_probe(name: &str, js: &str) -> String {
    let work = std::env::temp_dir().join(format!("json_stringify_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("创建工作目录失败");
    std::fs::write(work.join("probe.js"), js).unwrap();
    common::assert_e2e_matches_node(&work, "probe.js")
}

/// 键序：创建序 + 整数键前置升序 + 空串/前导零键归类。
#[test]
fn key_order_matches_node() {
    let out = run_probe(
        "key_order",
        r#"
console.log(JSON.stringify({ a: 1, b: 2 }));
console.log(JSON.stringify({ b: 2, a: 1 }));           // 创建序
console.log(JSON.stringify({ done: false, value: 'h' })); // 迭代结果对象形态
console.log(JSON.stringify({ 2: 'x', 1: 'y', a: 1 }));  // 整数键前置升序
console.log(JSON.stringify({ "": 1, "0": 2, "00": 3, "1": 4 }));
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], r#"{"a":1,"b":2}"#);
    assert_eq!(lines[1], r#"{"b":2,"a":1}"#);
    assert_eq!(lines[2], r#"{"done":false,"value":"h"}"#);
    assert_eq!(lines[3], r#"{"1":"y","2":"x","a":1}"#);
    assert_eq!(lines[4], r#"{"0":2,"1":4,"":1,"00":3}"#);
}

/// undefined/函数/符号值：对象整键剔除；数组元素占位 "null"；顶层 undefined。
#[test]
fn ignored_values_matches_node() {
    let out = run_probe(
        "ignored_values",
        r#"
console.log(JSON.stringify({ a: undefined, b: null, c: 3 }));
console.log(JSON.stringify({ f: function(){}, g: 1, h: undefined }));
console.log(JSON.stringify([1, undefined, function(){}, null]));
console.log(JSON.stringify(undefined) === undefined ? 'undef' : JSON.stringify(undefined));
console.log(JSON.stringify(function(){}) === undefined ? 'fn-undef' : 'x');
console.log(JSON.stringify(null));
console.log(JSON.stringify({ x: { y: [1, { z: undefined }] } }));
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], r#"{"b":null,"c":3}"#);
    assert_eq!(lines[1], r#"{"g":1}"#);
    assert_eq!(lines[2], r#"[1,null,null,null]"#);
    assert_eq!(lines[3], "undef");
    assert_eq!(lines[4], "fn-undef");
    assert_eq!(lines[5], "null");
    assert_eq!(lines[6], r#"{"x":{"y":[1,{}]}}"#);
}

/// 超字典阈值（40 键）对象保插入序；删除后重加落在末尾（V8 慢化语义）。
#[test]
fn dict_order_and_delete_readd_matches_node() {
    let out = run_probe(
        "dict_order",
        r#"
const big = {};
for (let i = 0; i < 40; i++) { const k = 'k' + (i < 10 ? '0' + i : i); big[k] = i; }
console.log(JSON.stringify(big));
const del = { a: 1, b: 2, c: 3 };
delete del.b;
del.b = 9; // 重加 → 键序末尾（Node：a,c,b）
console.log(JSON.stringify(del));
delete del.a;
console.log(JSON.stringify(del));
const again = { x: 1, y: 2 };
delete again.missing; // 删除不存在键不改序
again.z = 3;
console.log(JSON.stringify(again));
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        r#"{"k00":0,"k01":1,"k02":2,"k03":3,"k04":4,"k05":5,"k06":6,"k07":7,"k08":8,"k09":9,"k10":10,"k11":11,"k12":12,"k13":13,"k14":14,"k15":15,"k16":16,"k17":17,"k18":18,"k19":19,"k20":20,"k21":21,"k22":22,"k23":23,"k24":24,"k25":25,"k26":26,"k27":27,"k28":28,"k29":29,"k30":30,"k31":31,"k32":32,"k33":33,"k34":34,"k35":35,"k36":36,"k37":37,"k38":38,"k39":39}"#
    );
    assert_eq!(lines[1], r#"{"a":1,"c":3,"b":9}"#);
    assert_eq!(lines[2], r#"{"c":3,"b":9}"#);
    assert_eq!(lines[3], r#"{"x":1,"y":2,"z":3}"#);
}
