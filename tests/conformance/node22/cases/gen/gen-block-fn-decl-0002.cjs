// 回归保护用例（M5.2 修复轮）：**块内函数声明**在 sloppy 模式下提升到最近的函数作用域
// —— 块内可调用（含声明位置之前）、块外可访问、函数体内的块同样生效。
//
// 修复前 aluka 侧这些断言全部得到 `undefined`（`compile_stmt` 对 `Stmt::Function`
// 是空实现，提升收集只遍历直接子语句），Node 与修复后一致（本用例在门禁内与 Node
// 逐字节对拍）。
//
// 注意：**块执行前**引用块内函数名的差异属已知偏离，另置于
// `gen/deviations/gen-block-fn-decl-0001.cjs`，本用例不覆盖该点。
// 块内（**声明之前**调用）：Node 在块内提升该声明 → function + inner-ok。
{
  console.log('plain-block-in=' + typeof inner + ' call=' + inner());
  function inner() {
    return 'inner-ok';
  }
}
console.log('plain-block-after=' + typeof inner);
if (true) {
  console.log('if-block-in=' + typeof inIf + ' call=' + inIf());
  function inIf() {
    return 'if-ok';
  }
}
console.log('if-block-after=' + typeof inIf);

function outer() {
  for (let i = 0; i < 1; i++) {
    function inLoop() {
      return 'loop-ok';
    }
    console.log('loop-in=' + typeof inLoop + ' call=' + inLoop());
  }
  try {
    function inTry() {
      return 'try-ok';
    }
    console.log('try-in=' + typeof inTry + ' call=' + inTry());
  } catch (e) {
    console.log('try-catch=' + e.message);
  }
  return typeof inLoop + ',' + typeof inTry;
}
console.log('fn-body-blocks=' + outer());

// 提升后的块内函数引用外层变量（上值捕获）仍须正确。
function counter() {
  let n = 0;
  {
    function bump() {
      n += 1;
      return n;
    }
    bump();
    bump();
  }
  return typeof bump === 'function' ? bump() : 'missing';
}
console.log('upvalue=' + counter());
