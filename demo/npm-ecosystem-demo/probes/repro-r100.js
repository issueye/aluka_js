// 轮 100 缺陷最小复现：正则字符类（代理区/十六进制转义）、defineProperty 描述符
// 缺省值与访问器属性枚举顺序、with 语句（lodash template 编译产物依赖）。
// 注意：不带 'use strict'——with 在严格模式被禁，而 lodash template 的
// `new Function` 编译产物正是非严格体。
function t(label, fn) {
  try { console.log(label + '=' + JSON.stringify(fn())); }
  catch (e) { console.log(label + '=ERR ' + e.name + ': ' + e.message); }
}

// —— lodash words / stringSize 底层正则 ——
// reAsciiWord：否定字符类 + 十六进制转义范围（asciiWords 路径）
t('asciiWords', () => 'foo bar-baz'.match(/[^\x00-\x2f\x3a-\x40\x5b-\x60\x7b-\x7f]+/g));

// reHasUnicode：字符串拼出的字符类，含代理区 \ud800-\udfff（hasUnicode 判定）
const rsAstralRange = '\\ud800-\\udfff';
const rsComboRange = '\\u0300-\\u036f\\ufe20-\\ufe2f';
const rsVarRange = '\\ufe0e\\ufe0f';
const rsZWJ = '\\u200d';
const reHasUnicode = RegExp('[' + rsZWJ + rsAstralRange + rsComboRange + rsVarRange + ']');
t('hasUnicode_abc', () => reHasUnicode.test('abc'));
t('hasUnicode_astral', () => reHasUnicode.test('a\u{1f600}b'));

// reUnicode：代理区 | 组合记号（unicodeSize 的 exec 循环；pad 偏差根因）
const reUnicode = RegExp('[' + rsAstralRange + ']|[' + rsComboRange + ']', 'g');
t('unicodeSize_abc', () => {
  let n = 0;
  reUnicode.lastIndex = 0;
  while (reUnicode.exec('abc')) n++;
  return n;
});
t('unicodeSize_astral', () => {
  let n = 0;
  reUnicode.lastIndex = 0;
  while (reUnicode.exec('\u{1f600}')) n++;
  return n;
});

// reHasUnicodeWord：camelCase 分派到 unicodeWords 的判定
const reHasUnicodeWord = /[a-z][A-Z]|[A-Z]{2}[a-z]|[0-9][a-zA-Z]|[a-zA-Z][0-9]|[^a-zA-Z0-9 ]/;
t('hasUnicodeWord_fooBar', () => reHasUnicodeWord.test('fooBar'));
t('hasUnicodeWord_space', () => reHasUnicodeWord.test('foo bar'));

// —— defineProperty：描述符缺省值与访问器枚举顺序（zod 绑定探针根因）——
t('descDefaults', () => {
  const o = {};
  Object.defineProperty(o, 'x', { enumerable: true, get() { return 1; } });
  const d = Object.getOwnPropertyDescriptor(o, 'x');
  return [d.configurable, d.enumerable, d.writable === undefined];
});
t('dataDescDefaults', () => {
  const o = {};
  Object.defineProperty(o, 'x', { value: 5 });
  const d = Object.getOwnPropertyDescriptor(o, 'x');
  return [d.configurable, d.enumerable, d.writable];
});
t('accessorOrder', () => {
  const o = {};
  Object.defineProperty(o, 'a', { enumerable: true, get() { return 1; } });
  Object.defineProperty(o, 'b', { enumerable: true, get() { return 2; } });
  o.c = 3;
  return Object.keys(o).join(',');
});

// —— with 语句（lodash template 编译产物 `with(obj){...}` 依赖）——
// 已拆至 repro-with.js：解析器不支持 with 时整文件拒绝，会掩盖其余结论。
t('newFunction', () => new Function('a', 'return a + 1')(41));
console.log('REPRO_DONE');
