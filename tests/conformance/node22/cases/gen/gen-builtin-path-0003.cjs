// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-path
try { (async () => console.log(JSON.stringify(await (require("path").resolve("x") === require("path").resolve("x")))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
