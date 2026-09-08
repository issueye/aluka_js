//! M2 conformance 修复回归：对象字面量属性简写、`Buffer.from` 编码分流、
//! 正则 `replace`、Buffer 字符串拼接（对齐 node22 口径，见
//! 06/07/08/12 用例的会话证据）。

use aluka_compiler::compile_module;
use aluka_parser::parse;
use aluka_vm::Vm;

fn run_script(src: &str) -> Vec<String> {
    let program = parse(src);
    let module = compile_module(&program);
    module.verify().expect("模块通过 Verifier");
    let mut vm = Vm::new(0);
    vm.load_module(&module.serialize(), &module).expect("load");
    vm.run_module(&module).expect("run");
    vm.stdout_records
}

#[test]
fn m2_conf_fixes_match_node22() {
    let lines = run_script(
        // 属性简写 `{ port }`（08-http-agent 的 request options 形态）
        "const port = 8080;\n\
         const o = { host: '127.0.0.1', port, path: '/' };\n\
         console.log('s1: ' + o.port + ' ' + o.path + ' ' + o.host);\n\
         // Buffer.from(str, 'base64')：必须按编码解码（07-x509 DER 路径）\n\
         const der = Buffer.from('aGVsbG8=', 'base64');\n\
         console.log('s2: ' + der.length + ' ' + der.toString('utf8'));\n\
         // 正则 replace（g 标志替换全部；07 用例剥离 PEM 头尾）\n\
         console.log('s3: ' + 'a-b-c'.replace(/-/g, '+'));\n\
         console.log('s4: ' + 'x7y'.replace(/[0-9]/, '_'));\n\
         // Buffer 参与字符串拼接：ToPrimitive 走 toString（08 用例 body 累加）\n\
         console.log('s5: ' + Buffer.from('ok'));\n\
         console.log('s6: ' + 5 + ' ' + (5 + Buffer.from('!')));",
    );
    assert_eq!(
        lines,
        vec![
            "s1: 8080 / 127.0.0.1".to_owned(),
            "s2: 5 hello".to_owned(),
            "s3: a+b+c".to_owned(),
            "s4: x_y".to_owned(),
            "s5: ok".to_owned(),
            "s6: 5 5!".to_owned(),
        ],
        "属性简写/Buffer 编码/正则 replace/Buffer 拼接必须与 node22 一致"
    );
}

#[test]
fn buffer_from_utf8_string_stays_raw_bytes() {
    // 未指定编码（utf8）时保持字节语义：length 为字节长度
    let lines = run_script(
        "const b = Buffer.from('héllo');\n\
         console.log(b.length);",
    );
    assert_eq!(lines, vec!["6".to_owned()], "utf8 编码按字节计长");
}

#[test]
fn broadcast_channel_post_reaches_other_instances_only() {
    // 同频道广播：其它实例收到 message（参数 { data }），自身不回环
    let lines = run_script(
        "const b = new BroadcastChannel('chat');\n\
         b.on('message', (e) => console.log('got:' + e.data));\n\
         const b2 = new BroadcastChannel('chat');\n\
         b2.postMessage('hello');\n\
         console.log('sent');",
    );
    assert_eq!(
        lines,
        vec!["got:hello".to_owned(), "sent".to_owned()],
        "postMessage 必须广播到同频道其它实例（node22 06 用例口径）"
    );
}

/// 无分号 ASI（自动分号插入）：return 换行后语句必须独立解析（restricted
/// production）。raw-body readStream onEnd 的「if (complete) return 换行 if (err)
/// return done(err)」曾把下一行 if 当 return 表达式解析——err 检查丢失。
#[test]
fn asi_return_newline_terminates_statement() {
    let lines = run_script(
        "function readStream() {\n\
         var complete = false;\n\
         var log = [];\n\
         function done(err) { log.push('done:' + err); }\n\
         function onEnd(err) {\n\
           if (complete) return\n\
           if (err) return done(err)\n\
           log.push('cont')\n\
         }\n\
         onEnd(undefined);\n\
         onEnd('boom');\n\
         console.log(log.join(','));\n\
         }\n\
         readStream();",
    );
    assert_eq!(
        lines,
        vec!["cont,done:boom".to_owned()],
        "return 后换行必须 ASI 终止语句（node22 语义）"
    );
}

/// Error.prototype 独立于 Object.prototype：普通对象字面量 instanceof Error
/// 必须为 false，错误实例为 true（http-errors createError props 被误判的根因）。
#[test]
fn error_prototype_distinct_from_object_prototype() {
    let lines = run_script(
        "console.log('i1:' + ({} instanceof Error));\n\
         console.log('i2:' + (new Error('x') instanceof Error));\n\
         console.log('i3:' + (new TypeError('t') instanceof TypeError));\n\
         console.log('i4:' + ({} instanceof Object));",
    );
    assert_eq!(
        lines,
        vec![
            "i1:false".to_owned(),
            "i2:true".to_owned(),
            "i3:true".to_owned(),
            "i4:true".to_owned(),
        ],
        "Error.prototype 链必须与 Object.prototype 区分"
    );
}

/// Array 构造语义：单数值参数 → length=n 稀疏数组；多参/直调 → 元素数组。
/// raw-body new Array(arguments.length) 曾得空数组致回调参数全丢。
#[test]
fn array_ctor_single_numeric_length_semantics() {
    let lines = run_script(
        "console.log('a1:' + new Array(2).length);\n\
         console.log('a2:' + new Array(2, 3).join(','));\n\
         console.log('a3:' + Array(5).length);\n\
         var a = new Array(3); a[1] = 'x';\n\
         console.log('a4:' + a.length + ':' + a[1]);\n\
         console.log('a5:' + new Array('k').join(','));",
    );
    assert_eq!(
        lines,
        vec![
            "a1:2".to_owned(),
            "a2:2,3".to_owned(),
            "a3:5".to_owned(),
            "a4:3:x".to_owned(),
            "a5:k".to_owned(),
        ],
        "Array 构造（new/直调/多参/单非数值）必须对齐 node22"
    );
}
