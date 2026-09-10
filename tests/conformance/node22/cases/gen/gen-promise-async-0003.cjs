// conformance 生成用例（gen.mjs 产出，勿手改）：promise-async
try { console.log(JSON.stringify((async () => { const v = await Promise.resolve("v"); return v + "!" })())) } catch (e) { console.log("ERR", e.name) }
