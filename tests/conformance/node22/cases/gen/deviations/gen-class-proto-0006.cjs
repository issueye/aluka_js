// conformance 生成用例（gen.mjs 产出，勿手改）：class-proto
try { console.log(JSON.stringify((() => { class A { constructor() { this.x = 1 } } class B extends A { constructor() { super(); this.y = 2 } } const b = new B(); return [b.x, b.y] })())) } catch (e) { console.log("ERR", e.name) }
