// conformance 生成用例（gen.mjs 产出，勿手改）：map-set
try { console.log(JSON.stringify((() => { const m = new Map(); m.set("k", 7); m.set("k", 8); return m.get("k") })())) } catch (e) { console.log("ERR", e.name) }
