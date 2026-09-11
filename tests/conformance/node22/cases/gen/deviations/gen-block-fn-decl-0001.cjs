// 引擎偏离（隔离用例）：**块内函数声明**在 aluka 侧不可见。
//
// Node v22.23.1（本机实测，3/3 稳定）：
//   top-typeof=undefined
//   in-if-block-typeof=function call=inner-ok
//   in-plain-block-typeof=function
//   after-typeof=function
//
// aluka（修复前实测）：
//   top-typeof=undefined
//   in-if-block-typeof=undefined call=N/A     ← 块内也不可见（缺陷）
//   in-plain-block-typeof=undefined           ← 普通块同样（缺陷）
//   after-typeof=undefined
//
// 影响面：任何在 `if`/`for`/普通块内用函数声明写的真实代码（含本仓 M5.2 断连探针
// 首版）都会在块内拿到 `undefined`。属解析/编译期作用域绑定专项，见
// `.work/TODO/20260911/README.md` §11.9。
console.log('top-typeof=' + typeof inner);
if (true) {
  function inner() {
    return 'inner-ok';
  }
  console.log(
    'in-if-block-typeof=' + typeof inner +
    ' call=' + (typeof inner === 'function' ? inner() : 'N/A')
  );
}
{
  function inPlainBlock() {
    return 'plain-ok';
  }
  console.log('in-plain-block-typeof=' + typeof inPlainBlock);
}
console.log('after-typeof=' + typeof inner);
