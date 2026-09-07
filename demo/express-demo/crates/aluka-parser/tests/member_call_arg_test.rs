//! 回归：`f(a.b.c)` 成员链作为调用实参（http-errors 依赖 `createIsHttpErrorFunction(module.exports.HttpError)`）。

use aluka_parser::Parser;

#[test]
fn member_chain_argument_is_kept() {
    let src = "module.exports.isHttpError = createIsHttpErrorFunction(module.exports.HttpError);";
    let program = Parser::new(src).parse_program();
    assert_eq!(program.body.len(), 1, "单语句");
    let printed = format!("{:#?}", program.body[0]);
    // 实参中的 HttpError 成员读取不得被丢弃
    assert!(printed.contains("HttpError"), "实参成员链被丢弃: {printed}");
    let call_count = printed.matches("Call").count();
    assert!(call_count >= 1, "应含 Call 节点");
}
