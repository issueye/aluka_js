// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-stream
try { (async () => console.log(JSON.stringify(await ((() => { const { Readable } = require("stream"); return Readable.from([1, 2, 3]).readableLength >= 0 })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
