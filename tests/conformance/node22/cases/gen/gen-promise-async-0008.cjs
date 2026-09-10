// conformance 生成用例（gen.mjs 产出，勿手改）：promise-async
try { console.log(JSON.stringify((async () => { try { await Promise.reject(new Error("E")); } catch (e) { return e.name } })())) } catch (e) { console.log("ERR", e.name) }
