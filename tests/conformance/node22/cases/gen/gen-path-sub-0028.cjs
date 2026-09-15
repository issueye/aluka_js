// conformance 生成用例（gen.mjs 产出，勿手改）：path-sub
try { (async () => console.log(JSON.stringify(await ((() => { const f = require("path").win32.join; return f("a", "b") })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
