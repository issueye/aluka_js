// conformance 生成用例（gen.mjs 产出，勿手改）：map-set
try { console.log(JSON.stringify((() => { const s = new Set([1,2,3]); s.delete(2); return [...s] })())) } catch (e) { console.log("ERR", e.name) }
