// conformance 生成用例（gen.mjs 产出，勿手改）：error
try { console.log(JSON.stringify((() => { try { const s = new Set(); s.add(); } catch (e) { return "thrown" } })())) } catch (e) { console.log("ERR", e.name) }
