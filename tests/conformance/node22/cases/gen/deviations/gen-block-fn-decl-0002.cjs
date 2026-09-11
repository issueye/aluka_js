// 引擎偏离（隔离用例）：块内函数声明捕获**同块声明的 `let`/`const`** 时失效。
//
// 背景（`.work/TODO/20260911/README.md` §13.6）：块内函数声明的**可见性**已修复
// （块内可调用、块外可访问，见 `gen/gen-block-fn-decl-0002.cjs`）；但若该函数引用
// **同一个块内**用 `let`/`const` 声明的变量，函数内读到 `undefined`。
//
// 根因（编译期槽位模型）：
//   1. 块内函数声明被提升到**函数作用域**（其闭包在函数入口创建，捕获函数级预注册槽）；
//   2. 而块级 `let`/`const` 在 `codegen.rs` 中**总是分配新槽**（`block_depth > 0` 分支，
//      用于实现块级遮蔽），值写入的是**块级槽**；
//   3. 两者不是同一个槽 → 提升函数读到函数级槽的 `undefined`。
//
// 实测：
//   Node v22.23.1：same-block-capture=1
//   aluka（当前）：TypeError: Cannot read properties of undefined (reading 'push')
//
// 精确修法（已勘察，未实施）：把绑定动作从函数入口**下移到块入口**——块内函数模板
// 在编译期预编译入队（按收集序 / 按名匹配），`codegen.rs` 的 `Stmt::Block` 分支在
// 编译子语句前取出模板并 `MakeClosure` + `StoreLocal`，使闭包与块级 `let`/`const`
// 处于同一（块级）槽位语义下。
// 已尝试并**回退**的近似方案：让块级 `let`/`const` 复用函数级预注册槽——会破坏
// 块级遮蔽（`{ let x = 20 }` 泄漏到外层，`gen-lang-more-0014.cjs` 实测
// Node=10 / aluka=20），故不可取。
console.log('start');
{
  const rec = [];
  function start() {
    rec.push('ok');
    return rec.length;
  }
  console.log('same-block-capture=' + start());
}
