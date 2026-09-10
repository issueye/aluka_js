// conformance 生成用例（gen.mjs 产出，勿手改）：lang-more
try { console.log(JSON.stringify((() => { const o = new Proxy({}, { get: () => 42 }); return o.anything })())) } catch (e) { console.log("ERR", e.name) }
