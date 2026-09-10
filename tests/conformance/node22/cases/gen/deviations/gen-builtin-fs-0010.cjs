// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs/promises"); return fs.readFile("package.json").then(b => b.length > 0) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
