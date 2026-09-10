// 类声明在**函数作用域内**的回归用例（Node 22 对拍）。
//
// 改前：`Stmt::Class` 在常规语句编译路径里是空实现（codegen.rs 的
// `Stmt::Function | Stmt::Class => { ... }` 只做栈平衡，注释称「在 compile_module
// 中提取」），而提取只发生在**模块顶层**——于是函数/箭头/IIFE 体内的
// `class A { m() {} }` 既不绑定名字也不挂方法：
//   typeof A        → undefined（Node: function）
//   new A().m       → undefined（Node: function）
//   new B() instanceof A → false（Node: true）
// `new A()` 之所以「看似可用」，是因为 `new <未声明标识符>` 有宽松回退
// （`new TotallyUndeclared()` 同样不报错），并非绑定真的存在。
//
// 本用例只覆盖**已修复的作用域族**；以下构造仍是 parser/lexer 缺口，
// 刻意不含（会在两引擎上产生不同形态输出）：
//   class 字段 `x = 5`、`static m() {}`、`static F = 3`、`static {}`、
//   getter/setter、私有字段 `#p`、class 表达式作操作数。
'use strict';

const show = (label, fn) => {
  try {
    console.log(label + ':', fn());
  } catch (e) {
    console.log(label + ': ERR ' + e.name);
  }
};

// ---- 函数体内：名字绑定 + 方法 ----
show('fn-truthy', () => {
  function f() {
    class A { m() { return 1; } }
    return !!A;
  }
  return f();
});
show('fn-typeof', () => {
  function f() {
    class A {}
    return typeof A;
  }
  return f();
});
show('fn-method-call', () => {
  function f() {
    class A { m() { return 'ok'; } }
    return new A().m();
  }
  return f();
});
show('fn-method-typeof', () => {
  function f() {
    class A { m() {} }
    return typeof new A().m;
  }
  return f();
});
show('fn-multi-methods', () => {
  function f() {
    class A { a() { return 1; } b() { return 2; } }
    const o = new A();
    return o.a() + o.b();
  }
  return f();
});
show('fn-method-this', () => {
  function f() {
    class A { set() { this.v = 7; } get() { return this.v; } }
    const o = new A();
    o.set();
    return o.get();
  }
  return f();
});
show('fn-proto-own-names', () => {
  function f() {
    class A { m() {} }
    return JSON.stringify(Object.getOwnPropertyNames(A.prototype));
  }
  return f();
});
show('fn-proto-ctor-is-A', () => {
  function f() {
    class A {}
    return A.prototype.constructor === A;
  }
  return f();
});

// ---- 箭头 / IIFE / 嵌套 ----
show('arrow-method', () => {
  const f = () => {
    class A { m() { return 'am'; } }
    return new A().m();
  };
  return f();
});
show('iife-method', () => (function () {
  class A { m() { return 'iife'; } }
  return new A().m();
})());
show('nested-fn-class', () => {
  function outer() {
    function inner() {
      class A { m() { return 'deep'; } }
      return new A().m();
    }
    return inner();
  }
  return outer();
});

// ---- 继承（extends / super / instanceof / 原型链）----
show('fn-instanceof-self', () => {
  function f() {
    class A {}
    return new A() instanceof A;
  }
  return f();
});
show('fn-extends-instanceof', () => {
  function f() {
    class A {} class B extends A {}
    return new B() instanceof A;
  }
  return f();
});
show('fn-extends-self', () => {
  function f() {
    class A {} class B extends A {}
    return new B() instanceof B;
  }
  return f();
});
show('fn-extends-reverse', () => {
  function f() {
    class A {} class B extends A {}
    return new A() instanceof B;
  }
  return f();
});
show('fn-extends-inherit-method', () => {
  function f() {
    class A { m() { return 'a'; } } class B extends A {}
    return new B().m();
  }
  return f();
});
show('fn-extends-override', () => {
  function f() {
    class A { m() { return 'a'; } } class B extends A { m() { return 'b'; } }
    return new B().m();
  }
  return f();
});
show('fn-extends-ctor-super', () => {
  function f() {
    class A { constructor() { this.x = 1; } }
    class B extends A { constructor() { super(); this.y = 2; } }
    const b = new B();
    return b.x + ',' + b.y;
  }
  return f();
});
show('fn-extends-chain3', () => {
  function f() {
    class A {} class B extends A {} class C extends B {}
    return new C() instanceof A;
  }
  return f();
});
show('fn-extends-proto-chain', () => {
  function f() {
    class A {} class B extends A {}
    return Object.getPrototypeOf(B.prototype) === A.prototype;
  }
  return f();
});

// ---- 顶层仍须正常（防回归）----
class TopA { m() { return 'top'; } }
class TopB extends TopA {}
show('top-method', () => new TopA().m());
show('top-instanceof', () => new TopB() instanceof TopA);
show('top-proto-ctor', () => TopA.prototype.constructor === TopA);
