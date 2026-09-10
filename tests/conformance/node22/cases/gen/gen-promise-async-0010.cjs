// conformance 生成用例（gen.mjs 产出，勿手改）：promise-async
try { console.log(JSON.stringify((async () => { for (const v of [1,2]) { await Promise.resolve(v) } return "loop-done" })())) } catch (e) { console.log("ERR", e.name) }
