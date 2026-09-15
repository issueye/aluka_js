// conformance 生成用例（gen.mjs 产出，勿手改）：path-sub
try { (async () => console.log(JSON.stringify(await ((() => { const f = require("path").posix.basename; return f("/a/b/c.txt", ".txt") })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
