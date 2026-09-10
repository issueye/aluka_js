// conformance 生成用例（gen.mjs 产出，勿手改）：lang-more
try { console.log(JSON.stringify((() => { function* g(){ const x = yield 1; yield x * 2 } const it = g(); it.next(); return it.next(5).value })())) } catch (e) { console.log("ERR", e.name) }
