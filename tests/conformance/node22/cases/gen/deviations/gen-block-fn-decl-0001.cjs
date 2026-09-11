// 引擎偏离（隔离用例）：块内函数声明在**块执行前**的可见性不同。
//
// 主缺陷已于本轮修复（块内原来完全不可见 → 现在块内可调用、块外可访问）；
// 仅余以下细粒度差异：
//
//   Node v22.23.1：块内函数声明的绑定在**函数入口**初始化为 `undefined`，块执行时
//     才赋值 → 块执行**前**引用该名字得到 `undefined`（不抛错）；
//   aluka（修复后）：提升编译在**函数入口**即 `MakeClosure` + `StoreLocal` →
//     块执行前引用即为 `function`。
//
// 实测：
//   Node : before-block-typeof=undefined  after-block-typeof=function
//   aluka: before-block-typeof=function   after-block-typeof=function  ← 仅首行不同
//
// 影响面：仅在「块执行前引用块内函数名」时可见（如函数入口处做特性探测），方向为
// 更宽松（提前可见），不产生崩溃。精确对齐需把绑定动作从函数入口下移到**块入口**
// （块内模板预编译 + 块内绑定），属编译期绑定时机专项，见
// `.work/TODO/20260911/README.md` §12.6。
console.log('before-block-typeof=' + typeof inner);
if (true) {
  function inner() {
    return 'inner-ok';
  }
}
console.log('after-block-typeof=' + typeof inner);
