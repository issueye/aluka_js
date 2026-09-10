// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-os
try { (async () => console.log(JSON.stringify(await (typeof require("os").hostname()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
