// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-os
try { (async () => console.log(JSON.stringify(await (require("os").tmpdir().length > 0))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
