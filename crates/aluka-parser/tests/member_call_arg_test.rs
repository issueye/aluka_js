//! 回归：成员赋值 + 成员链实参的调用不得丢失实参。
//!
//! http-errors 的 `module.exports.isHttpError = createIsHttpErrorFunction(module.exports.HttpError)`
//! 曾被编译为 CALL count 0（实参 `module.exports.HttpError` 整体被丢弃），
//! 导致依赖包加载失败。

use aluka_parser::Parser;

#[test]
fn member_assign_with_member_chain_argument_keeps_call_args() {
    let src =
        "module.exports.isHttpError = createIsHttpErrorFunction(module.exports.HttpError);";
    let program = Parser::new(src).parse_program();
    assert_eq!(program.body.len(), 1, "单语句");
    let printed = format!("{:#?}", program.body[0]);
    // 实参中的 HttpError 成员读取不得被丢弃
    assert!(
        printed.matches("HttpError").count() >= 2,
        "实参成员链被丢弃: {printed}"
    );
}
