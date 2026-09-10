// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-stream
try { (async () => console.log(JSON.stringify(await ((() => { const { Writable } = require("stream"); const w = new Writable({ write(chunk, enc, cb) { cb() } }); w.write("x"); w.end(); return new Promise(r => w.on("finish", () => r("w-done"))) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
