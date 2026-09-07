/*---
esid: local-tagged-template
description: 标记模板字面量（tagged template）
---*/

// 基本：tag 接收 TemplateStringsArray + 插值。
function tag(s, v) {
  return s[0] + "|" + v + "|" + s[1];
}
assert.sameValue(tag`a${1 + 1}b`, "a|2|b", "tagged basic");

// 多插值。
function join(s, a, b, c) {
  return s[0] + a + s[1] + b + s[2] + c + s[3];
}
assert.sameValue(join`${1},${2},${3}`, "1,2,3", "tagged multi interp");

// cooked 处理转义，strings.raw 保留原文。
function cooked(s) {
  return s[0];
}
function raw(s) {
  return s.raw[0];
}
assert.sameValue(cooked`\n`, "\n", "cooked escape");
assert.sameValue(raw`\n`, "\\n", "raw preserved");

// 转义 ${ 不产生伪插值。
function esc(s) {
  return s.raw[0] + "|" + s[0];
}
assert.sameValue(esc`\${foo}`, "\\${foo}|${foo}", "escaped ${");

// 成员访问 tag：this 绑定接收者。
var obj = { n: 7, tag: function (s) { return this.n + s[0]; } };
assert.sameValue(obj.tag`hi`, "7hi", "member tag this");
